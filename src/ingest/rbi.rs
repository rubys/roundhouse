//! Tapioca RBIs as a typed gem boundary.
//!
//! `sorbet/rbi/gems/<name>@<version>.rbi` is what Tapioca generated
//! from the installed gem: its classes and modules, their ancestry, and
//! a `def` per public method, most with a `sig`. That is a declaration
//! of the gem's surface, and reading it is the difference between "the
//! receiver is a gem class, so nothing can be said" and "the receiver
//! is a `Money::Bank::Base`, and `#exchange_with` answers a `Money`".
//!
//! Only declarations are read; there are no bodies to read. The reader
//! is lenient where [`super::sorbet_sig`] is strict, and the leniency
//! is always toward `untyped`:
//!
//! * a `sig` the type grammar cannot fully read does not drop the
//!   method (the method EXISTS, that is what the RBI says), it types
//!   the pieces it cannot read as `untyped`;
//! * a method with no `sig` is `(untyped...) -> untyped`;
//! * `void` is `untyped`, not `nil`: sorbet's `void` says "do not use
//!   the result", which is not a claim that the result is nil.
//!
//! Nothing here reaches the analysis until the analyzer resolves the
//! written class names against the classes it actually knows (see
//! `analyze::registry::gem_boundary`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ruby_prism::Node;

use crate::effect::EffectSet;
use crate::gem_boundary::{GemBoundary, GemClass};
use crate::gems::{fate_of, GemFate, Lockfile};
use crate::ident::{ClassId, Symbol};
use crate::ty::{Param, ParamKind, Ty};
use crate::vfs::Vfs;

use super::sorbet_sig::{
    collect_type_aliases, constant_path_name, def_parameters, is_sig_call, qualify, sorbet_ty,
    symbol_name,
};
use super::util::constant_id_str;

/// Where a tree's RBIs live: `sorbet/rbi/gems` under the tree, or under
/// the directory its `Gemfile.lock` really lives in. The second matters
/// for a tree that is assembled from a checkout by linking (the lockfile
/// is a symlink to the real one, and `sorbet/` is not copied): the RBIs
/// sit beside the real lockfile.
fn rbi_dir<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Option<PathBuf> {
    let direct = dir.join("sorbet/rbi/gems");
    if vfs.is_dir(&direct) {
        return Some(direct);
    }
    let lock = vfs.canonical(&dir.join("Gemfile.lock"))?;
    let beside = lock.parent()?.join("sorbet/rbi/gems");
    vfs.is_dir(&beside).then_some(beside)
}

/// Read the RBIs of the locked gems the analyzer has no model for.
///
/// Rails' own components, the stdlib gems and the tooling gems are
/// skipped: the first two are already the analyzer's catalog (a second,
/// partial, description of them would only disagree), and the last never
/// enters the analysis at all.
pub fn load_gem_boundary<V: Vfs + ?Sized>(vfs: &V, dir: &Path, lock: &Lockfile) -> GemBoundary {
    let mut boundary = GemBoundary::default();
    let Some(root) = rbi_dir(vfs, dir) else { return boundary };
    let Ok(entries) = vfs.read_dir(&root) else { return boundary };

    // gem name -> the RBI file to read. A gem locked at one version can
    // have RBIs for several (a stale one left behind); the locked
    // version's wins, otherwise the highest-sorting one.
    let mut chosen: HashMap<String, (bool, PathBuf)> = HashMap::new();
    for path in entries {
        let Some(file) = path.file_name().and_then(|f| f.to_str()) else { continue };
        let Some(stem) = file.strip_suffix(".rbi") else { continue };
        let Some((name, version)) = stem.split_once('@') else { continue };
        if lock.is_in_repo(name) || !matches!(fate_of(name), GemFate::Unknown | GemFate::Modeled) {
            continue;
        }
        let Some(locked) = lock.version_of(name) else { continue };
        let exact = version == locked || version.starts_with(&format!("{locked}-"));
        match chosen.get(name) {
            Some((was_exact, was)) if (*was_exact, was) >= (exact, &path) => {}
            _ => {
                chosen.insert(name.to_string(), (exact, path));
            }
        }
    }

    let mut files: Vec<(String, PathBuf)> = chosen.into_iter().map(|(n, (_, p))| (n, p)).collect();
    files.sort();
    for (gem, path) in files {
        if let Ok(source) = vfs.read(&path) {
            read_rbi(&source, &gem, &mut boundary);
            boundary.namespaces.entry(gem).or_default();
        }
    }
    let mut namespaces = std::mem::take(&mut boundary.namespaces);
    for (id, class) in &boundary.classes {
        let content = !class.instance_methods.is_empty()
            || !class.class_methods.is_empty()
            || !class.constants.is_empty()
            || class.parent.is_some()
            || !class.includes.is_empty()
            || !class.extends.is_empty()
            || class.dynamic;
        if content {
            if let Some(names) = namespaces.get_mut(&class.gem) {
                names.insert(id.0.as_str().to_string());
            }
        }
    }
    boundary.namespaces = namespaces;
    boundary
}

/// Declarations of one RBI file, merged into `out`. A class reopened
/// across files (or twice in one) accumulates: the first parent wins,
/// methods and ancestry union.
pub fn read_rbi(source: &[u8], gem: &str, out: &mut GemBoundary) {
    let result = ruby_prism::parse(source);
    let node = result.node();
    let Some(program) = node.as_program_node() else { return };
    let statements = program.statements().body().iter().collect::<Vec<_>>();
    let aliases = collect_type_aliases(&statements, &HashMap::new());
    let mut reader = Reader { gem, out };
    reader.walk(&statements, None, false, &aliases);
}

struct Reader<'a> {
    gem: &'a str,
    out: &'a mut GemBoundary,
}

/// The name a `class`/`module` header declares, given its enclosing
/// scope. `class ::Foo` is `Foo` wherever it sits.
fn declared_name(scope: Option<&str>, path: &Node<'_>) -> String {
    let cbase = path.as_constant_path_node().is_some_and(|p| {
        let mut root = p.parent();
        loop {
            match root {
                None => return true,
                Some(node) => match node.as_constant_path_node() {
                    Some(inner) => root = inner.parent(),
                    None => return false,
                },
            }
        }
    });
    let written = constant_path_name(path);
    if cbase { written } else { qualify(scope, &written) }
}

impl Reader<'_> {
    fn class_entry(&mut self, name: &str, is_module: bool) -> &mut GemClass {
        let gem = self.gem;
        let entry = self.out.classes.entry(ClassId(Symbol::new(name))).or_default();
        if entry.gem.is_empty() {
            entry.gem = gem.to_string();
            entry.is_module = is_module;
        }
        entry
    }

    fn walk(
        &mut self,
        statements: &[Node<'_>],
        scope: Option<&str>,
        singleton: bool,
        aliases: &HashMap<String, Ty>,
    ) {
        // The `sig` directly above a member applies to it; remembered
        // by index because prism nodes are not `Clone`.
        let mut pending: Option<usize> = None;
        for (index, statement) in statements.iter().enumerate() {
            let sig = pending.take().map(|i| &statements[i]);
            if let Some(class) = statement.as_class_node() {
                let name = declared_name(scope, &class.constant_path());
                let parent = class.superclass();
                let parent_name = parent.as_ref().map(constant_path_name).filter(|n| !n.is_empty());
                let entry = self.class_entry(&name, false);
                match (parent_name, parent.is_some()) {
                    (Some(p), _) => {
                        if entry.parent.is_none() {
                            entry.parent = Some(ClassId(Symbol::new(&p)));
                        }
                    }
                    // `class X < Struct.new(:a)` and the like: an
                    // ancestor exists and cannot be named.
                    (None, true) => entry.dynamic = true,
                    (None, false) => {}
                }
                if let Some(body) = class.body().and_then(|b| b.as_statements_node()) {
                    let inner = body.body().iter().collect::<Vec<_>>();
                    let aliases = collect_type_aliases(&inner, aliases);
                    self.walk(&inner, Some(&name), false, &aliases);
                }
                continue;
            }
            if let Some(module) = statement.as_module_node() {
                let name = declared_name(scope, &module.constant_path());
                self.class_entry(&name, true);
                if let Some(body) = module.body().and_then(|b| b.as_statements_node()) {
                    let inner = body.body().iter().collect::<Vec<_>>();
                    let aliases = collect_type_aliases(&inner, aliases);
                    self.walk(&inner, Some(&name), false, &aliases);
                }
                continue;
            }
            if let Some(single) = statement.as_singleton_class_node() {
                if let Some(body) = single.body().and_then(|b| b.as_statements_node()) {
                    self.walk(&body.body().iter().collect::<Vec<_>>(), scope, true, aliases);
                }
                continue;
            }
            let Some(scope) = scope else {
                // Top-level: only a `sig` carries over to the next
                // statement; there is no class to declare into.
                pending = statement
                    .as_call_node()
                    .is_some_and(|c| is_sig_call(&c))
                    .then_some(index);
                continue;
            };
            if let Some(def) = statement.as_def_node() {
                self.declare_def(scope, &def, sig, singleton, aliases);
                continue;
            }
            if let Some(alias) = statement.as_alias_method_node() {
                if let (Some(new), Some(old)) =
                    (symbol_name(&alias.new_name()), symbol_name(&alias.old_name()))
                {
                    self.declare_alias(scope, &new, &old, singleton);
                }
                continue;
            }
            if let Some(write) = statement.as_constant_write_node() {
                if !aliases.contains_key(constant_id_str(&write.name())) {
                    let name = Symbol::new(constant_id_str(&write.name()));
                    self.class_entry(scope, false).constants.push(name);
                }
                continue;
            }
            let Some(call) = statement.as_call_node() else { continue };
            if is_sig_call(&call) {
                pending = Some(index);
                continue;
            }
            if call.receiver().is_some() {
                continue;
            }
            let name = constant_id_str(&call.name());
            let args: Vec<Node<'_>> = call
                .arguments()
                .map(|a| a.arguments().iter().collect())
                .unwrap_or_default();
            match name {
                "include" | "prepend" | "extend" => {
                    for arg in &args {
                        let target = constant_path_name(arg);
                        if target.is_empty() {
                            continue;
                        }
                        let entry = self.class_entry(scope, false);
                        let list = if name == "extend" && !singleton {
                            &mut entry.extends
                        } else if singleton && name == "include" {
                            // `class << self; include M` -- the same fact.
                            &mut entry.extends
                        } else {
                            &mut entry.includes
                        };
                        list.push(ClassId(Symbol::new(&target)));
                    }
                }
                "attr_reader" | "attr_writer" | "attr_accessor" => {
                    let declared = sig
                        .and_then(|s| read_returns(s, true, aliases))
                        .unwrap_or(Ty::Untyped);
                    for arg in &args {
                        let Some(attr) = symbol_name(arg) else { continue };
                        if name != "attr_writer" {
                            self.insert(scope, &attr, singleton, reader_fn(declared.clone()));
                        }
                        if name != "attr_reader" {
                            self.insert(
                                scope,
                                &format!("{attr}="),
                                singleton,
                                writer_fn(declared.clone()),
                            );
                        }
                    }
                }
                "alias_method" => {
                    if let [new, old] = args.as_slice() {
                        if let (Some(new), Some(old)) = (symbol_name(new), symbol_name(old)) {
                            self.declare_alias(scope, &new, &old, singleton);
                        }
                    }
                }
                // `private def x` / `private_class_method def self.x`:
                // the def is the member, the wrapper only its
                // visibility.
                "private" | "protected" | "public" | "module_function" | "private_class_method" => {
                    for arg in &args {
                        if let Some(def) = arg.as_def_node() {
                            self.declare_def(scope, &def, sig, singleton, aliases);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn insert(&mut self, scope: &str, name: &str, class_side: bool, ty: Ty) {
        let entry = self.class_entry(scope, false);
        let table = if class_side { &mut entry.class_methods } else { &mut entry.instance_methods };
        table.insert(Symbol::new(name), ty);
    }

    fn declare_def(
        &mut self,
        scope: &str,
        def: &ruby_prism::DefNode<'_>,
        sig: Option<&Node<'_>>,
        singleton: bool,
        aliases: &HashMap<String, Ty>,
    ) {
        let name = constant_id_str(&def.name());
        let class_side = singleton || def.receiver().is_some();
        // A `def other.x` declares onto something that is not this
        // class; only `def self.x` is ours.
        if def.receiver().is_some_and(|r| r.as_self_node().is_none()) {
            return;
        }
        if name == "method_missing" && !class_side {
            self.class_entry(scope, false).dynamic = true;
        }
        let ty = lenient_signature(sig, def, !class_side, aliases);
        self.insert(scope, name, class_side, ty);
    }

    /// `alias_method :new, :old` gives `new` the type `old` already
    /// has; an `old` this file has not declared (it was inherited, or
    /// declared later) leaves `new` untyped rather than guessed.
    fn declare_alias(&mut self, scope: &str, new: &str, old: &str, class_side: bool) {
        let entry = self.class_entry(scope, false);
        let table = if class_side { &mut entry.class_methods } else { &mut entry.instance_methods };
        let ty = table.get(&Symbol::new(old)).cloned().unwrap_or_else(untyped_fn);
        table.insert(Symbol::new(new), ty);
    }
}

fn untyped_fn() -> Ty {
    Ty::Fn {
        params: vec![Param { name: Symbol::new("args"), ty: Ty::Untyped.into(), kind: ParamKind::Rest }].into(),
        block: None,
        ret: std::sync::Arc::new(Ty::Untyped),
        effects: EffectSet::pure(),
    }
}

fn reader_fn(ret: Ty) -> Ty {
    Ty::Fn { params: Vec::new().into(), block: None, ret: std::sync::Arc::new(ret), effects: EffectSet::pure() }
}

fn writer_fn(ty: Ty) -> Ty {
    Ty::Fn {
        params: vec![Param { name: Symbol::new("value"), ty: ty.clone().into(), kind: ParamKind::Required }].into(),
        block: None,
        ret: std::sync::Arc::new(ty),
        effects: EffectSet::pure(),
    }
}

/// The links of a `sig { ... }` chain, outermost first.
fn sig_chain<'a>(sig: &Node<'a>) -> Vec<ruby_prism::CallNode<'a>> {
    let mut out = Vec::new();
    let Some(call) = sig.as_call_node() else { return out };
    let Some(block) = call.block().and_then(|b| b.as_block_node()) else { return out };
    let Some(body) = block.body().and_then(|b| b.as_statements_node()) else { return out };
    let mut link = body.body().iter().next();
    while let Some(node) = link {
        let Some(call) = node.as_call_node() else { break };
        link = call.receiver();
        out.push(call);
    }
    out
}

/// The declared return of a `sig`, `None` when it declares none or the
/// type is outside the grammar. `void` is not a declared return here.
fn read_returns(sig: &Node<'_>, self_is_instance: bool, aliases: &HashMap<String, Ty>) -> Option<Ty> {
    for call in sig_chain(sig) {
        if constant_id_str(&call.name()) == "returns" {
            let argument = call.arguments()?.arguments().iter().next()?;
            return sorbet_ty(&argument, self_is_instance, aliases);
        }
    }
    None
}

/// `(params) -> ret` for a `def`, from its `sig` where there is one.
///
/// The def supplies the parameter names, order and kinds (Ruby is the
/// authority on what `x` is); the sig supplies types, and any type it
/// does not supply or this grammar cannot read is `untyped`.
fn lenient_signature(
    sig: Option<&Node<'_>>,
    def: &ruby_prism::DefNode<'_>,
    self_is_instance: bool,
    aliases: &HashMap<String, Ty>,
) -> Ty {
    let mut declared: HashMap<String, Ty> = HashMap::new();
    let mut ret = Ty::Untyped;
    if let Some(sig) = sig {
        for call in sig_chain(sig) {
            match constant_id_str(&call.name()) {
                "returns" => {
                    if let Some(argument) = call.arguments().and_then(|a| a.arguments().iter().next()) {
                        ret = sorbet_ty(&argument, self_is_instance, aliases).unwrap_or(Ty::Untyped);
                    }
                }
                "params" => {
                    let Some(arguments) = call.arguments() else { continue };
                    for argument in arguments.arguments().iter() {
                        let Some(hash) = argument.as_keyword_hash_node() else { continue };
                        for element in hash.elements().iter() {
                            let Some(assoc) = element.as_assoc_node() else { continue };
                            let Some(name) = symbol_name(&assoc.key()) else { continue };
                            let ty = sorbet_ty(&assoc.value(), self_is_instance, aliases)
                                .unwrap_or(Ty::Untyped);
                            declared.insert(name, ty);
                        }
                    }
                }
                // `void`, and every modifier (`override`, `abstract`,
                // `type_parameters`, ...), say nothing readable here.
                _ => {}
            }
        }
    }
    let positional_tail = def.parameters().is_some_and(|p| p.posts().iter().next().is_some());
    let listed = match def.parameters() {
        None => Some(Vec::new()),
        Some(_) if positional_tail => None,
        Some(parameters) => def_parameters(&parameters),
    };
    let params = match listed {
        Some(listed) => listed
            .into_iter()
            .map(|(name, kind)| {
                let ty = declared.remove(&name).unwrap_or(Ty::Untyped);
                Param { name: Symbol::new(&name), ty: ty.into(), kind }
            })
            .collect(),
        // A parameter list this reader does not take apart: accept
        // anything rather than assert an arity.
        None => vec![Param { name: Symbol::new("args"), ty: Ty::Untyped.into(), kind: ParamKind::Rest }],
    };
    Ty::Fn { params: params.into(), block: None, ret: std::sync::Arc::new(ret), effects: EffectSet::pure() }
}
