//! `Dry::Struct` classes lowered into the plain Ruby they stand for.
//!
//! ```ruby
//! class CreateRefund < BaseResponse          # BaseResponse < Dry::Struct
//!   attribute :id, Types::Coercible::String
//!   attribute? :description, Types::Strict::String.optional
//! end
//! ```
//!
//! Like `T::Struct`, a `Dry::Struct` is a class generator: `attribute`
//! is the constructor and the reader. The emitted tree has no dry-struct,
//! so each class becomes a reader per attribute and an
//! `initialize(attributes = {})` that does what `Dry::Struct.new` does
//! with a Hash: transform its keys by the nearest `transform_keys`, take
//! each attribute's key, coerce or check it by its type, raise
//! `Dry::Struct::Error` when a required key is missing or a value does
//! not fit. A subclass's own attributes come after its parent's, as the
//! schema inherits.
//!
//! Types mean what dry-types makes them, checked against the gem: a bare
//! name is strict under `Dry.Types()`; `Coercible::`, `Strict::`,
//! `Params::` and `JSON::` scalars, dates and decimals; `Array.of`,
//! `Hash.schema`, `Instance(X)`, another struct class, `A | B`,
//! `Constructor(K) { }` and `.constructor { }`, `.optional`, `.enum`,
//! `.meta`, and `.default` (a value shared from where it is written, or
//! a block called each time); a constant holding any of these. A nested
//! `attribute ... do` becomes the `Owner::Name` struct dry-struct defines,
//! `attributes_from` inlines another struct's attributes, and
//! `attribute :name?` is `attribute? :name`.
//!
//! A class with anything else (`transform_types`, an app's own
//! `attribute` override) is left as it was and ledgered, and so is every
//! class that shares its `Dry::Struct` root or builds one that does: a
//! struct is all its attributes or none, and a hierarchy all its structs
//! or none. The date and decimal coercions need stdlib only the CRuby and
//! JRuby trees load; `project` reports them for any other target.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

use super::dry_types::{
    BareNames, DryType, Gen, Scope, coerced, dry_type, resolve,
    resolve_in, types_modules, types_path,
};
use super::{IngestError, survey};

struct Attribute {
    name: Symbol,
    /// `attribute?`: the key may be absent.
    omittable: bool,
    ty: DryType,
}

struct StructClass {
    attributes: Vec<Attribute>,
    /// The `transform_keys` call this class declares, if any.
    transform_keys: Option<Expr>,
}

impl StructClass {
    fn structs(&self) -> Vec<String> {
        let mut out = Vec::new();
        for a in &self.attributes {
            a.ty.structs(&mut out);
        }
        out
    }
}

pub(super) fn lower_dry_structs(app: &mut App, sources: &[crate::span::SourceFile]) {
    let types_modules = types_modules(sources);
    let mut parent_of = struct_parents(&app.library_classes);
    if parent_of.is_empty() {
        return;
    }
    let nested_owner = expand_nested(app, &mut parent_of, &types_modules);
    inline_attributes_from(app, &parent_of);
    let mut names: HashSet<String> =
        app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
    // The app's own classes, which an `Array.of` may name in place of a type.
    let classes: HashSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .chain(app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()))
        .collect();
    let constants: HashMap<String, Expr> = app
        .library_classes
        .iter()
        .flat_map(|lc| {
            lc.constants
                .iter()
                .map(move |(n, v)| (format!("{}::{}", lc.name.0.as_str(), n.as_str()), v.clone()))
        })
        .collect();
    names.extend(constants.keys().cloned());

    // Read every struct's declarations and build its methods; a class
    // either part fails is refused here, before any hierarchy is lowered.
    let mut read: HashMap<String, StructClass> = HashMap::new();
    let mut built: HashMap<String, (Vec<crate::dialect::MethodDef>, Vec<(Symbol, Expr)>)> = HashMap::new();
    let mut refused: HashSet<String> = HashSet::new();
    // What each refused class names as a struct type.
    let mut refused_names: Vec<String> = Vec::new();
    for lc in &app.library_classes {
        let name = lc.name.0.as_str();
        if !parent_of.contains_key(name) {
            continue;
        }
        let scope = Scope {
            types_modules: &types_modules,
            owner: name,
            names: &names,
            structs: &parent_of,
            constants: &constants,
            classes: &classes,
            depth: 0,
        };
        let s = match read_struct(lc, &scope) {
            Ok(s) => s,
            Err(reason) => {
                survey::record(&IngestError::Unsupported {
                    file: name.to_string(),
                    message: format!("Dry::Struct not lowered: {reason}"),
                });
                refused.insert(name.to_string());
                refused_names.extend(named_structs(lc, &scope));
                continue;
            }
        };
        let source = synthesized_source(name, &s, parent_of[name].is_none());
        let (parsed, diags) = crate::ingest::prism::scope(|| {
            crate::ingest::ingest_library_classes(source.as_bytes(), "<dry_struct>")
        });
        match parsed {
            Ok(classes) if diags.is_empty() => {
                let mut methods = Vec::new();
                let mut generated = Vec::new();
                for c in classes {
                    methods.extend(c.methods);
                    generated.extend(c.constants);
                }
                built.insert(name.to_string(), (methods, generated));
            }
            Ok(_) => {
                survey::record_synthesis_failure(name.to_string(), "Dry::Struct lowering", &diags);
                refused.insert(name.to_string());
                refused_names.extend(s.structs());
            }
            Err(err) => {
                survey::record(&err);
                refused.insert(name.to_string());
                refused_names.extend(s.structs());
            }
        }
        read.insert(name.to_string(), s);
    }
    // All or nothing per hierarchy. Lowering a root makes it an ordinary
    // known class, so a refused descendant would no longer reach the
    // unknown `Dry::Struct` and its attribute calls would read as
    // missing methods rather than as the gap they are. A struct that
    // builds another struct needs that one's hierarchy lowered too, and
    // a nested struct goes with its owner: an owner kept on the gem names
    // it as a type, which it must then still be.
    let root_of = |name: &str| -> String {
        let mut cur = name.to_string();
        while let Some(Some(parent)) = parent_of.get(&cur) {
            cur = parent.clone();
        }
        cur
    };
    let mut refused_roots: HashSet<String> =
        refused.iter().chain(&refused_names).map(|n| root_of(n)).collect();
    loop {
        let before = refused_roots.len();
        for (name, s) in &read {
            if s.structs().iter().any(|dep| refused_roots.contains(&root_of(dep))) {
                refused_roots.insert(root_of(name));
            }
        }
        for (nested, owner) in &nested_owner {
            if refused_roots.contains(&root_of(owner)) {
                refused_roots.insert(root_of(nested));
            }
        }
        if refused_roots.len() == before {
            break;
        }
    }
    let lowered = |name: &str| -> bool { !refused_roots.contains(&root_of(name)) };

    let mut any = false;
    for lc in &mut app.library_classes {
        let name = lc.name.0.as_str().to_string();
        if !parent_of.contains_key(&name) || !lowered(&name) {
            continue;
        }
        let Some((mut methods, generated)) = built.remove(&name) else { continue };
        lc.unknown_calls.retain(|call| !is_struct_declaration(call));
        // Dry-type constants are stripped app-wide after both lowerings;
        // keep generated non-type constants (shared defaults).
        lc.constants.extend(generated);
        lc.origin = Some(crate::dialect::LibraryClassOrigin::DryStruct);
        methods.append(&mut lc.methods);
        lc.methods = methods;
        // The parent by the name it resolved to, not as written: `Base`
        // inside `module Shop` is `Shop::Base`, which is how every later
        // pass looks a class up.
        lc.parent = parent_of[&name].clone().map(|p| crate::ident::ClassId(Symbol::from(p.as_str())));
        any = true;
    }
    // Lowered constructors raise / rescue `Dry::Struct::Error`. Emit the
    // stand-in (empty `class Dry::Struct` + nested `Error`) whenever
    // anything lowered — including when some hierarchies stay unlowered.
    // The empty class shell is load-critical under partial refusal: nesting
    // only `Error` makes the Ruby emitter open `module Struct`, and
    // `class X < Dry::Struct` then TypeErrors. The shell carries no
    // `attribute` methods; refused class-body DSL is not replayed onto a
    // modeled parent (`replays_foreign_class_body`).
    //
    // With that shell, nothing left on dry-types can run: refused subclasses
    // do not replay gem DSL, and lowered bodies bake coercions into
    // `initialize`. Refused files may still `require_relative` a `Types`
    // module (Const refs in dropped calls), so dry-type constants are
    // stripped after both lowerings (`dry_initializer::drop_type_constants`)
    // — otherwise `Types.Constructor` / `Types::Coercible::…` crash the
    // boot of the whole `models.rb` load, including lowered siblings.
    if any {
        app.library_classes.extend(error_classes());
    }
}

/// Each struct class and the struct parent it inherits from (None for a
/// direct `Dry::Struct` subclass).
fn struct_parents(classes: &[LibraryClass]) -> HashMap<String, Option<String>> {
    let names: HashSet<String> = classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    loop {
        let before = parent_of.len();
        for lc in classes {
            let name = lc.name.0.as_str();
            if parent_of.contains_key(name) || lc.is_module {
                continue;
            }
            let Some(parent) = &lc.parent else { continue };
            let parent = parent.0.as_str().trim_start_matches("::");
            if parent == "Dry::Struct" {
                parent_of.insert(name.to_string(), None);
            } else if let Some(resolved) = resolve(name, parent, &names) {
                if parent_of.contains_key(&resolved) {
                    parent_of.insert(name.to_string(), Some(resolved));
                }
            }
        }
        if parent_of.len() == before {
            break;
        }
    }
    parent_of
}

/// `attribute :amount do ... end` defines `Owner::Amount`, a direct
/// `Dry::Struct` subclass that keeps the owner's key transform, and the
/// attribute takes that class; with `Types::Array` the class is singular
/// (`attribute :items, Types::Array do` defines `Owner::Item`) and the
/// attribute is an Array of it. Each nested class is a struct like any
/// other, so its own blocks nest the same way.
fn expand_nested(
    app: &mut App,
    parent_of: &mut HashMap<String, Option<String>>,
    types_modules: &[(String, BareNames)],
) -> HashMap<String, String> {
    let mut owner_of: HashMap<String, String> = HashMap::new();
    let mut queue: Vec<String> = parent_of.keys().cloned().collect();
    queue.sort();
    while let Some(owner) = queue.pop() {
        let Some(index) = app.library_classes.iter().position(|lc| lc.name.0.as_str() == owner) else {
            continue;
        };
        let transform = effective_transform(&app.library_classes, parent_of, &owner);
        let existing: HashSet<String> =
            app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
        let mut created = Vec::new();
        for call in &mut app.library_classes[index].unknown_calls {
            let ExprNode::Send { recv: None, method, args, block, .. } = &mut *call.node else { continue };
            if !matches!(method.as_str(), "attribute" | "attribute?") {
                continue;
            }
            let Some(ExprNode::Lambda { body, .. }) = block.as_ref().map(|b| &*b.node) else { continue };
            let (array, attr) = match args.as_slice() {
                [name] => (None, name),
                [name, ty] => match &*ty.node {
                    ExprNode::Const { path }
                        if types_path(path, &owner, types_modules, &existing)
                            .is_some_and(|rest| matches!(rest.as_slice(), [_, "Array"] | ["Array"])) =>
                    {
                        (Some(ty.clone()), name)
                    }
                    _ => continue,
                },
                _ => continue,
            };
            let ExprNode::Lit { value: Literal::Sym { value: attr } } = &*attr.node else { continue };
            let const_name = if array.is_some() {
                crate::naming::camelize(&crate::naming::singularize(attr.as_str()))
            } else {
                crate::naming::camelize(attr.as_str())
            };
            let full = format!("{owner}::{const_name}");
            if existing.contains(&full) || created.iter().any(|c: &LibraryClass| c.name.0.as_str() == full) {
                continue;
            }
            let mut unknown_calls: Vec<Expr> = match &*body.node {
                ExprNode::Seq { exprs } => exprs.clone(),
                _ => vec![body.clone()],
            };
            if let Some(t) = &transform {
                unknown_calls.insert(0, t.clone());
            }
            created.push(LibraryClass {
                name: crate::ident::ClassId(Symbol::from(full.as_str())),
                is_module: false,
                parent: Some(crate::ident::ClassId(Symbol::from("Dry::Struct"))),
                parent_span: crate::span::Span::synthetic(),
                includes: Vec::new(),
                include_spans: Vec::new(),
                methods: Vec::new(),
                nullable_columns: Vec::new(),
                origin: None,
                constants: Vec::new(),
                unknown_calls,
                class_ivar_initializers: Vec::new(),
            });
            let span = call.span;
            let class_ref = Expr::new(
                span,
                ExprNode::Const {
                    path: std::iter::once(Symbol::from(""))
                        .chain(full.split("::").map(Symbol::from))
                        .collect(),
                },
            );
            let ty = match array {
                Some(array) => Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(array),
                        method: Symbol::from("of"),
                        args: vec![class_ref],
                        block: None,
                        parenthesized: true,
                    },
                ),
                None => class_ref,
            };
            *args = vec![args[0].clone(), ty];
            *block = None;
        }
        for c in created {
            let name = c.name.0.as_str().to_string();
            parent_of.insert(name.clone(), None);
            owner_of.insert(name.clone(), owner.clone());
            queue.push(name);
            app.library_classes.push(c);
        }
    }
    owner_of
}

/// `attributes_from X` declares `X`'s attributes, inherited ones first,
/// in its place. Inlined when every constant they name is spelled from
/// the top (`::...`), so it reads the same from the new class; otherwise
/// it stays, and the class is refused for it.
fn inline_attributes_from(app: &mut App, parent_of: &HashMap<String, Option<String>>) {
    let names: HashSet<String> = app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
    // Sources first, round by round; a cycle is left to refuse.
    loop {
        let calls_of = |name: &str| -> Option<Vec<Expr>> {
            let mut chain = Vec::new();
            let mut cur = Some(name.to_string());
            while let Some(n) = cur {
                chain.push(n.clone());
                cur = parent_of.get(&n).cloned().flatten();
            }
            let mut out = Vec::new();
            for n in chain.iter().rev() {
                let lc = app.library_classes.iter().find(|lc| lc.name.0.as_str() == n)?;
                for call in &lc.unknown_calls {
                    let ExprNode::Send { recv: None, method, .. } = &*call.node else { continue };
                    match method.as_str() {
                        "attribute" | "attribute?" => out.push(call.clone()),
                        // Not inlined yet: wait for it, so its keys come along.
                        "attributes_from" => return None,
                        _ => {}
                    }
                }
            }
            out.iter().all(absolute_constants).then_some(out)
        };
        let mut rewrites: Vec<(usize, usize, Vec<Expr>)> = Vec::new();
        for (ci, lc) in app.library_classes.iter().enumerate() {
            let owner = lc.name.0.as_str();
            if !parent_of.contains_key(owner) {
                continue;
            }
            for (ui, call) in lc.unknown_calls.iter().enumerate() {
                let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else { continue };
                if method.as_str() != "attributes_from" {
                    continue;
                }
                let [ExprNode::Const { path }] = args.iter().map(|a| &*a.node).collect::<Vec<_>>()[..] else { continue };
                let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                let Some(source) = resolve_in(owner, &written, &names).filter(|n| parent_of.contains_key(n)) else {
                    continue;
                };
                if let Some(calls) = calls_of(&source) {
                    rewrites.push((ci, ui, calls));
                }
            }
        }
        if rewrites.is_empty() {
            break;
        }
        for (ci, ui, calls) in rewrites.into_iter().rev() {
            app.library_classes[ci].unknown_calls.splice(ui..=ui, calls);
        }
    }
}

/// Every constant in `expr` is spelled from the top level.
fn absolute_constants(expr: &Expr) -> bool {
    let own = match &*expr.node {
        ExprNode::Const { path } => path.first().is_some_and(|s| s.as_str().is_empty()),
        _ => true,
    };
    let mut ok = own;
    expr.node.for_each_child(&mut |c| ok &= absolute_constants(c));
    ok
}

/// The `transform_keys` call that applies to `name`: its own, or its
/// nearest struct ancestor's.
fn effective_transform(
    classes: &[LibraryClass],
    parent_of: &HashMap<String, Option<String>>,
    name: &str,
) -> Option<Expr> {
    let mut cur = Some(name.to_string());
    while let Some(n) = cur {
        let own = classes.iter().find(|lc| lc.name.0.as_str() == n).and_then(|lc| {
            lc.unknown_calls.iter().rev().find(|call| {
                matches!(&*call.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "transform_keys")
            })
        });
        if let Some(call) = own {
            return Some(call.clone());
        }
        cur = parent_of.get(&n).cloned().flatten();
    }
    None
}

/// Empty `Dry::Struct` plus nested `Error`, which the lowered
/// constructors raise and an application may rescue. Nothing else of
/// dry-struct is left. The empty class (not module) shell is required so
/// nest emit opens `class Struct` and refused `class X < Dry::Struct`
/// keeps Class ancestry.
fn error_classes() -> Vec<LibraryClass> {
    let source = "module Dry\n  class Struct\n    class Error < TypeError\n    end\n  end\nend\n";
    crate::ingest::ingest_library_classes(source.as_bytes(), "<dry_struct>")
        .expect("the Dry::Struct::Error stand-in parses")
}

fn is_struct_declaration(call: &Expr) -> bool {
    matches!(&*call.node, ExprNode::Send { recv: None, method, .. }
        if matches!(method.as_str(), "attribute" | "attribute?" | "transform_keys"))
}

/// `Dry::Struct` class methods the lowering does not model.
const DRY_STRUCT_DSL: &[&str] = &[
    "attributes",
    "attributes_from",
    "transform_types",
    "schema",
    "abstract",
    "input",
    "constructor_type",
    "load",
];

/// The struct classes a declaration names as types, read loosely: every
/// constant anywhere in its `attribute` arguments that resolves to one.
/// Used for a refused class, which the gem keeps, so whatever it names
/// as a struct type must stay a `Dry::Struct` too.
fn named_structs(lc: &LibraryClass, scope: &Scope<'_>) -> Vec<String> {
    fn walk(expr: &Expr, scope: &Scope<'_>, out: &mut Vec<String>) {
        if let ExprNode::Const { path } = &*expr.node {
            let written: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
            let name = written.iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("::");
            let full = if written.first() == Some(&"") {
                scope.names.contains(&name).then_some(name)
            } else {
                resolve_in(scope.owner, &name, scope.names)
            };
            match full {
                Some(full) if scope.structs.contains_key(&full) => out.push(full),
                // A constant holding a type names what its value names.
                Some(full) if scope.depth <= 8 => {
                    if let Some(value) = scope.constants.get(&full) {
                        let holder = full.rsplit_once("::").map_or("", |(h, _)| h);
                        walk(value, &Scope { owner: holder, depth: scope.depth + 1, ..*scope }, out);
                    }
                }
                _ => {}
            }
        }
        expr.node.for_each_child(&mut |c| walk(c, scope, out));
    }
    let mut out = Vec::new();
    for call in &lc.unknown_calls {
        if let ExprNode::Send { recv: None, method, args, .. } = &*call.node
            && matches!(method.as_str(), "attribute" | "attribute?")
        {
            for arg in args {
                walk(arg, scope, &mut out);
            }
        }
    }
    out
}

fn read_struct(lc: &LibraryClass, scope: &Scope<'_>) -> Result<StructClass, String> {
    let mut attributes = Vec::new();
    let mut transform_keys = None;
    for call in &lc.unknown_calls {
        // Anything that is not a modeled dry-struct declaration refuses:
        // `prepend` / `extend` (including from `class << self`) wrap
        // `attribute` and would silently drop validation policy if ignored.
        let ExprNode::Send { recv, method, args, block, .. } = &*call.node else {
            return Err("a non-declaration expression in the class body".into());
        };
        if matches!(method.as_str(), "prepend" | "extend") {
            return Err(format!("`{}` in the class body", method.as_str()));
        }
        if recv.is_some() {
            return Err(format!("`{}` with a receiver in the class body", method.as_str()));
        }
        match method.as_str() {
            "transform_keys" => {
                if !args.is_empty() || block.is_none() {
                    return Err("transform_keys without a block".into());
                }
                transform_keys = Some(call.clone());
            }
            "attribute" | "attribute?" => {
                if block.is_some() {
                    return Err("a nested `attribute ... do` struct".into());
                }
                let [name, ty] = args.as_slice() else {
                    return Err("an attribute with options".into());
                };
                let ExprNode::Lit { value: Literal::Sym { value: name } } = &*name.node else {
                    return Err("an attribute named by an expression".into());
                };
                let ty = dry_type(ty, scope).ok_or_else(|| {
                    format!("attribute `{}` type `{}`", name.as_str(), crate::emit::ruby::emit_expr(ty))
                })?;
                // `attribute :name?` is dry-struct's other spelling of
                // `attribute? :name`.
                let (name, suffixed) = match name.as_str().strip_suffix('?') {
                    Some(bare) => (Symbol::from(bare), true),
                    None => (name.clone(), false),
                };
                attributes.push(Attribute { name, omittable: suffixed || method.as_str() == "attribute?", ty });
            }
            // The rest of dry-struct's class DSL changes the schema or the
            // constructor; dropping it would lower a different struct.
            other if DRY_STRUCT_DSL.contains(&other) => {
                return Err(format!("`{other}` in the class body"));
            }
            other => {
                return Err(format!("`{other}` in the class body"));
            }
        }
    }
    Ok(StructClass { attributes, transform_keys })
}

fn synthesized_source(owner: &str, s: &StructClass, is_root: bool) -> String {
    let mut extra = Gen {
        owner: owner.to_string(),
        coercion_error: "Dry::Struct::Error",
        constraint_error: "Dry::Struct::Error",
        ..Gen::default()
    };
    let mut body = String::new();
    if is_root || s.transform_keys.is_some() {
        let keys = match &s.transform_keys {
            // The declared call, sent to the attributes Hash instead.
            Some(call) => {
                let mut call = call.clone();
                if let ExprNode::Send { recv, .. } = &mut *call.node {
                    *recv = Some(Expr::new(
                        call.span,
                        ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("attributes") },
                    ));
                }
                crate::emit::ruby::emit_expr(&call)
            }
            None => "attributes".to_string(),
        };
        body.push_str(&format!("  def self.dry_struct_keys(attributes)\n    {keys}\n  end\n\n"));
    }
    if is_root {
        body.push_str(
            "  def initialize(attributes = {})\n    dry_struct_assign(self.class.dry_struct_keys(attributes))\n  end\n\n",
        );
    }
    for a in &s.attributes {
        body.push_str(&format!("  def {0}\n    @{0}\n  end\n\n", a.name.as_str()));
    }
    let mut assign_body = String::new();
    if !is_root {
        assign_body.push_str("    super(attributes)\n");
    }
    for a in &s.attributes {
        let key = a.name.as_str();
        let value = format!("attributes[:{key}]");
        let assign = coerced(&mut extra, &a.ty, &value, &format!(":{key}"));
        let missing = match (&a.ty.default, a.omittable) {
            (Some(default), _) => extra.missing_value(default),
            (None, true) => "nil".to_string(),
            (None, false) => format!(
                "raise(Dry::Struct::Error, \"[#{{self.class}}.new] :{key} is missing in Hash input\")"
            ),
        };
        assign_body.push_str(&format!(
            "    @{key} = if attributes.key?(:{key})\n      {assign}\n    else\n      {missing}\n    end\n"
        ));
    }
    let mut out = String::new();
    let name = owner.rsplit("::").next().unwrap_or(owner);
    out.push_str(&format!("class {name}\n"));
    for constant in &extra.constants {
        out.push_str(&format!("  {constant}\n"));
    }
    if !extra.constants.is_empty() {
        out.push('\n');
    }
    for helper in &extra.helpers {
        out.push_str(helper);
        out.push('\n');
    }
    out.push_str(&body);
    out.push_str("  private\n\n  def dry_struct_assign(attributes)\n");
    out.push_str(&assign_body);
    out.push_str("    self\n  end\nend\n");
    out
}
