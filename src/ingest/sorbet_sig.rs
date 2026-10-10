//! sorbet-runtime `sig` blocks as signature seeds.
//!
//! An app that adopted sorbet-runtime has already written down what
//! inference has to work out, and it wrote it in the place inference
//! runs out: at the boundary to a gem the catalog does not model. The
//! shape recurs — a call into an unmodeled base class returns a result
//! object, so the value is untyped, and the `sig` on the next method
//! down names the type that was lost.
//!
//! This reads those annotations into `app.rbs_signatures`, the table
//! `sig/**/*.rbs` already feeds, so there is no new consumer: the
//! analyzer overlays it onto its catalog, and the Spinel lane's RBS
//! sidecar is emitted from the same place.
//!
//! What is read is deliberately narrow — method signatures, and only
//! the type grammar below. A `sig` this module cannot represent is
//! dropped whole and inference handles that method as before, the way
//! `spinel_rbs_extract` drops RBS it cannot represent. Dropping is a
//! no-op; guessing is not.

use std::collections::HashMap;

use ruby_prism::Node;

use crate::ident::{ClassId, Symbol};
use crate::effect::EffectSet;
use crate::ty::{Param, ParamKind, Ty};

use super::util::constant_id_str;

/// Method signatures declared with `sig` in one Ruby file, keyed by the
/// enclosing class's qualified name.
pub fn ingest_sorbet_signatures(source: &[u8]) -> HashMap<ClassId, HashMap<Symbol, Ty>> {
    ingest_sorbet_declarations(source).0
}

/// Signatures plus the names declared `sig { abstract… }` per class or
/// module. An abstract method is a declaration with no implementation to
/// carry: the includer supplies it.
pub fn ingest_sorbet_declarations(
    source: &[u8],
) -> (
    HashMap<ClassId, HashMap<Symbol, Ty>>,
    HashMap<ClassId, std::collections::HashSet<Symbol>>,
) {
    let result = ruby_prism::parse(source);
    let node = result.node();
    let Some(program) = node.as_program_node() else {
        return (HashMap::new(), HashMap::new());
    };
    let mut out: HashMap<ClassId, HashMap<Symbol, Ty>> = HashMap::new();
    let mut abstracts = HashMap::new();
    let mut sides = MethodSides::new();
    walk(
        &program.statements().body().iter().collect::<Vec<_>>(),
        None,
        false,
        &HashMap::new(),
        &HashMap::new(),
        &mut out,
        &mut abstracts,
        &mut sides,
        source,
    );
    // The table holds one signature per (class, name), read by both the
    // instance and the singleton side. A class that defines `def
    // self.to_entity(record)` beside `def to_entity(lookup: nil)` has
    // two different signatures under one key, and whichever was read
    // last used to type both bodies: the class method's `record` came out
    // as the instance method's keyword `lookup`. A signature that would
    // apply to the wrong side is worse than none, so a name defined on
    // both sides keeps neither and is inferred as before.
    for (class, methods) in out.iter_mut() {
        methods.retain(|name, _| {
            !sides
                .get(&(class.clone(), name.clone()))
                .is_some_and(|(instance, singleton)| *instance && *singleton)
        });
    }
    (out, abstracts)
}

/// Which sides of a class define each method name: `(instance,
/// singleton)`.
type MethodSides = HashMap<(ClassId, Symbol), (bool, bool)>;

/// `in_singleton_class` is true while walking a `class << self` body.
/// The methods there are the enclosing class's SINGLETON methods even
/// though their `def` carries no receiver, which is the one thing a
/// reader must not get wrong: `T.self_type` means the class object
/// there, not an instance of it.
fn walk(
    statements: &[Node<'_>],
    scope: Option<&str>,
    in_singleton_class: bool,
    aliases: &HashMap<String, Ty>,
    rbs_aliases: &crate::rbs::AliasTable,
    out: &mut HashMap<ClassId, HashMap<Symbol, Ty>>,
    abstracts: &mut HashMap<ClassId, std::collections::HashSet<Symbol>>,
    sides: &mut MethodSides,
    source: &[u8],
) {
    // The `sig` immediately above a `def` is the one that applies to
    // it; anything else between them (a comment is not a statement,
    // but another call is) means the pairing is not ours to guess.
    // Prism nodes are not `Clone`, so the pending `sig` is remembered
    // by its index in this body rather than by value.
    let mut pending: Option<usize> = None;
    for (index, statement) in statements.iter().enumerate() {
        if let Some(class) = statement.as_class_node() {
            let name = qualify(scope, &constant_path_name(&class.constant_path()));
            if let Some(body) = class.body() {
                if let Some(body) = body.as_statements_node() {
                    let statements = body.body().iter().collect::<Vec<_>>();
                    let superclass = class.superclass().map(|s| constant_path_name(&s));
                    if superclass.as_deref().is_some_and(is_struct_superclass) {
                        collect_struct_properties(&statements, &name, out);
                    }
                    if superclass.as_deref() == Some("T::Enum") {
                        collect_enum_surface(&statements, &name, out);
                    }
                    // `Name = T.type_alias { … }` in this body names a
                    // TYPE, and a `sig` below it refers to that name.
                    // Without resolving it the reader produced a
                    // `Ty::Class` for a class that does not exist, and
                    // every use of the annotated parameter dispatched
                    // against nothing. Collected before the walk so the
                    // order of alias and `sig` in the file does not
                    // matter; an enclosing scope's aliases stay visible.
                    let inner = collect_type_aliases(&statements, aliases);
                    let inner_rbs = scoped_rbs_aliases(source, class.location(), &statements, rbs_aliases);
                    walk(&statements, Some(&name), false, &inner, &inner_rbs, out, abstracts, sides, source);
                }
            }
            pending = None;
            continue;
        }
        if let Some(module) = statement.as_module_node() {
            let name = qualify(scope, &constant_path_name(&module.constant_path()));
            if let Some(body) = module.body() {
                if let Some(body) = body.as_statements_node() {
                    let statements = body.body().iter().collect::<Vec<_>>();
                    let inner = collect_type_aliases(&statements, aliases);
                    let inner_rbs = scoped_rbs_aliases(source, module.location(), &statements, rbs_aliases);
                    walk(&statements, Some(&name), false, &inner, &inner_rbs, out, abstracts, sides, source);
                }
            }
            pending = None;
            continue;
        }
        // `class << self` — its `def`s are the enclosing class's
        // singleton methods, and their `sig`s were invisible: the walk
        // only descended into class and module bodies, so a whole
        // singleton surface declared this way read as undeclared.
        // Same scope, because that is the class the methods belong to
        // and this table does not separate the two sides anyway.
        if let Some(singleton) = statement.as_singleton_class_node() {
            if let Some(body) = singleton.body() {
                if let Some(body) = body.as_statements_node() {
                    walk(&body.body().iter().collect::<Vec<_>>(), scope, true, aliases, rbs_aliases, out, abstracts, sides, source);
                }
            }
            pending = None;
            continue;
        }
        if let Some(def) = statement.as_def_node() {
            if let Some(scope) = scope {
                let entry = sides
                    .entry((ClassId(Symbol::new(scope)), Symbol::new(constant_id_str(&def.name()))))
                    .or_default();
                if in_singleton_class || def.receiver().is_some() {
                    entry.1 = true;
                } else {
                    entry.0 = true;
                }
            }
            if let (Some(sig), Some(scope)) = (pending.take(), scope) {
                if !in_singleton_class && def.receiver().is_none() && sig_is_abstract(&statements[sig]) {
                    abstracts
                        .entry(ClassId(Symbol::new(scope)))
                        .or_default()
                        .insert(Symbol::new(constant_id_str(&def.name())));
                }
                if let Some(ty) = signature_ty(&statements[sig], &def, in_singleton_class, aliases) {
                    out.entry(ClassId(Symbol::new(scope)))
                        .or_default()
                        .insert(Symbol::new(constant_id_str(&def.name())), ty);
                }
            }
            else if let Some(scope) = scope {
                if let Some(ty) = rbs_comment_signature(source, &def, in_singleton_class, rbs_aliases) {
                    out.entry(ClassId(Symbol::new(scope)))
                        .or_default()
                        .insert(Symbol::new(constant_id_str(&def.name())), ty);
                }
            }
            continue;
        }
        pending = match statement.as_call_node() {
            Some(call) if is_sig_call(&call) => Some(index),
            _ => None,
        };
    }
}

/// `T::Struct` / `T::ImmutableStruct` — sorbet-runtime's typed value
/// object. `const :name, String` declares a reader whose type the app
/// has written down, and a domain-heavy app keeps most of its types in
/// exactly these classes.
fn is_struct_superclass(name: &str) -> bool {
    matches!(name, "T::Struct" | "T::ImmutableStruct" | "T::InexactStruct")
}

/// The readers (and, for `prop`, writers) a struct body declares.
///
/// `default:` / `factory:` / `extra:` ride along on the same call and
/// say nothing about the type, so they are skipped rather than read.
/// A declaration whose type is outside the grammar drops on its own,
/// leaving the others — unlike a method signature, where a half-read
/// `params` would mistype the method.
fn collect_struct_properties(
    statements: &[Node<'_>],
    class_name: &str,
    out: &mut HashMap<ClassId, HashMap<Symbol, Ty>>,
) {
    for statement in statements {
        let Some(call) = statement.as_call_node() else { continue };
        if call.receiver().is_some() {
            continue;
        }
        let method = call.name();
        let writable = match constant_id_str(&method) {
            "const" => false,
            "prop" => true,
            _ => continue,
        };
        let Some(arguments) = call.arguments() else { continue };
        let mut arguments = arguments.arguments().iter();
        let Some(name) = arguments.next().and_then(|n| symbol_name(&n)) else { continue };
        let Some(ty) = arguments.next().and_then(|n| sorbet_ty(&n, true, &HashMap::new())) else { continue };
        let reader = Ty::Fn {
            params: Vec::new().into(),
            block: None,
            ret: std::sync::Arc::new(ty.clone()),
            effects: EffectSet::pure(),
        };
        let class = out.entry(ClassId(Symbol::new(class_name))).or_default();
        class.insert(Symbol::new(&name), reader);
        if writable {
            class.insert(
                Symbol::new(&format!("{name}=")),
                Ty::Fn {
                    params: vec![Param {
                        name: Symbol::new("value"),
                        ty: ty.clone().into(),
                        kind: ParamKind::Required,
                    }].into(),
                    block: None,
                    ret: std::sync::Arc::new(ty),
                    effects: EffectSet::pure(),
                },
            );
        }
    }
}

/// `T::Enum`'s own surface: the members are constants holding
/// instances (the library ingest reads those), and every instance
/// answers `serialize` with the value it was constructed from.
///
/// The value type comes from the members themselves rather than being
/// assumed to be String: `new("fill")` says String, `new(1)` says
/// Integer, and an enum whose members disagree says nothing.
fn collect_enum_surface(
    statements: &[Node<'_>],
    class_name: &str,
    out: &mut HashMap<ClassId, HashMap<Symbol, Ty>>,
) {
    let mut value_ty: Option<Ty> = None;
    for statement in statements {
        let Some(call) = statement.as_call_node() else { continue };
        let method = call.name();
        if constant_id_str(&method) != "enums" || call.receiver().is_some() {
            continue;
        }
        let Some(block) = call.block().and_then(|b| b.as_block_node()) else { continue };
        let Some(body) = block.body().and_then(|b| b.as_statements_node()) else { continue };
        for member in body.body().iter() {
            let Some(write) = member.as_constant_write_node() else { continue };
            let Some(new_call) = write.value().as_call_node() else { continue };
            let name = new_call.name();
            if constant_id_str(&name) != "new" {
                continue;
            }
            let ty = new_call
                .arguments()
                .and_then(|a| a.arguments().iter().next())
                .and_then(|a| literal_ty(&a));
            match (&value_ty, ty) {
                (None, Some(ty)) => value_ty = Some(ty),
                (Some(seen), Some(ty)) if *seen == ty => {}
                // Members that disagree, or a member built from
                // something that is not a literal: say nothing.
                _ => return,
            }
        }
    }
    let Some(value_ty) = value_ty else { return };
    let enum_ty = Ty::Class { id: ClassId(Symbol::new(class_name)), args: Vec::new().into() };
    let nullary = |ret: Ty| Ty::Fn {
        params: Vec::new().into(),
        block: None,
        ret: std::sync::Arc::new(ret),
        effects: EffectSet::pure(),
    };
    let class = out.entry(ClassId(Symbol::new(class_name))).or_default();
    class.insert(Symbol::new("serialize"), nullary(value_ty.clone()));
    class.insert(
        Symbol::new("deserialize"),
        Ty::Fn {
            params: vec![Param {
                name: Symbol::new("value"),
                ty: value_ty.into(),
                kind: ParamKind::Required,
            }].into(),
            block: None,
            ret: std::sync::Arc::new(enum_ty.clone()),
            effects: EffectSet::pure(),
        },
    );
    class.insert(Symbol::new("values"), nullary(Ty::Array { elem: std::sync::Arc::new(enum_ty) }));
}

/// The type of a literal in a member's constructor call.
fn literal_ty(node: &Node<'_>) -> Option<Ty> {
    if node.as_string_node().is_some() {
        return Some(Ty::Str);
    }
    if node.as_integer_node().is_some() {
        return Some(Ty::Int);
    }
    if node.as_symbol_node().is_some() {
        return Some(Ty::Sym);
    }
    None
}

pub(super) fn symbol_name(node: &Node<'_>) -> Option<String> {
    let symbol = node.as_symbol_node()?;
    Some(String::from_utf8_lossy(symbol.value_loc()?.as_slice()).into_owned())
}

/// Does the `sig` chain carry the `abstract` modifier?
fn sig_is_abstract(sig: &Node<'_>) -> bool {
    let Some(call) = sig.as_call_node() else { return false };
    let Some(block) = call.block() else { return false };
    let Some(block) = block.as_block_node() else { return false };
    let Some(body) = block.body() else { return false };
    let Some(body) = body.as_statements_node() else { return false };
    let mut link = body.body().iter().next();
    while let Some(node) = link {
        let Some(call) = node.as_call_node() else { return false };
        if constant_id_str(&call.name()) == "abstract" {
            return true;
        }
        link = call.receiver();
    }
    false
}

pub(super) fn is_sig_call(call: &ruby_prism::CallNode<'_>) -> bool {
    let name = call.name();
    constant_id_str(&name) == "sig" && call.receiver().is_none() && call.block().is_some()
}

/// `Foo`, `Foo::Bar` — the class's own path as written.
pub(super) fn constant_path_name(node: &Node<'_>) -> String {
    if let Some(read) = node.as_constant_read_node() {
        return constant_id_str(&read.name()).to_string();
    }
    if let Some(path) = node.as_constant_path_node() {
        let mut name = match path.parent() {
            Some(parent) => format!("{}::", constant_path_name(&parent)),
            None => String::new(),
        };
        if let Some(child) = path.name() {
            name.push_str(constant_id_str(&child));
        }
        return name;
    }
    String::new()
}

pub(super) fn qualify(scope: Option<&str>, name: &str) -> String {
    match scope {
        Some(scope) if !name.is_empty() => format!("{scope}::{name}"),
        _ => name.to_string(),
    }
}

/// The `Ty::Fn` a `sig { … }` declares for the `def` below it, or
/// `None` when any part of it is outside the grammar.
fn signature_ty(
    sig: &Node<'_>,
    def: &ruby_prism::DefNode<'_>,
    in_singleton_class: bool,
    aliases: &HashMap<String, Ty>,
) -> Option<Ty> {
    // `def self.build` is singleton-side, `def build` instance-side —
    // which is what decides whether `T.self_type` is readable. See
    // `sorbet_ty`. Inside `class << self` a receiverless `def` is
    // singleton-side too, and reading its `self` as an instance would
    // be a wrong type rather than a missing one.
    let self_is_instance = def.receiver().is_none() && !in_singleton_class;
    let call = sig.as_call_node()?;
    let block = call.block()?;
    let block = block.as_block_node()?;
    let body = block.body()?;
    let body = body.as_statements_node()?;
    let declaration = body.body().iter().next()?;

    let mut declared: HashMap<String, Ty> = HashMap::new();
    let mut returns: Option<Ty> = None;
    let mut saw_return_clause = false;
    // `params(...).returns(X)`, `void`, `override.returns(X)`,
    // `abstract.params(...).void` — a chain of calls, read from the
    // outermost inward.
    let mut link = Some(declaration);
    while let Some(node) = link {
        let call = node.as_call_node()?;
        let name = call.name();
        match constant_id_str(&name) {
            "returns" => {
                saw_return_clause = true;
                let argument = call.arguments()?.arguments().iter().next()?;
                if returns.is_none() {
                    returns = Some(sorbet_ty(&argument, self_is_instance, aliases)?);
                }
            }
            "void" => {
                saw_return_clause = true;
                if returns.is_none() {
                    returns = Some(Ty::Nil);
                }
            }
            "params" => {
                for argument in call.arguments()?.arguments().iter() {
                    let hash = argument.as_keyword_hash_node()?;
                    for element in hash.elements().iter() {
                        let assoc = element.as_assoc_node()?;
                        let key = assoc.key();
                        let key = key.as_symbol_node()?;
                        let name = String::from_utf8_lossy(key.value_loc()?.as_slice()).into_owned();
                        declared.insert(name, sorbet_ty(&assoc.value(), self_is_instance, aliases)?);
                    }
                }
            }
            // Modifiers carry no type: `override`, `overridable`,
            // `abstract`, `final`, `checked(:never)`, `type_parameters`.
            // `type_parameters(:U)` only DECLARES the variables a
            // `T.type_parameter(:U)` below names; `bind(Foo)` says what
            // `self` is inside a block the method takes.
            "override" | "overridable" | "abstract" | "final" | "checked" | "type_parameters"
            | "bind" => {}
            _ => return None,
        }
        link = call.receiver();
    }
    if !saw_return_clause {
        return None;
    }

    let mut params = Vec::new();
    let mut block = None;
    if let Some(parameters) = def.parameters() {
        for (name, kind) in def_parameters(&parameters)? {
            let mut ty = declared.remove(&name)?;
            // The block parameter is the method's block: `T.proc…`, or
            // `T.nilable(T.proc…)` when a caller may omit it. Recorded
            // both as the block and as the parameter, the way the RBS
            // reader does, holding the callable itself.
            if kind == ParamKind::Block {
                if let Ty::Union { variants } = &ty {
                    if let [only @ Ty::Fn { .. }, Ty::Nil] = variants.as_slice() {
                        ty = only.clone();
                    }
                }
                if matches!(ty, Ty::Fn { .. }) {
                    block = Some(std::sync::Arc::new(ty.clone()));
                }
            }
            params.push(Param { name: Symbol::new(&name), ty: ty.into(), kind });
        }
    }
    // A name in the sig that the def does not have means the pairing is
    // wrong, not that the extra entry is harmless.
    if !declared.is_empty() {
        return None;
    }

    Some(Ty::Fn {
        params: params.into(),
        block,
        ret: std::sync::Arc::new(returns?),
        effects: EffectSet::pure(),
    })
}

/// The def's own parameters, in order, with the kind Ruby gives them —
/// the sig names them all the same way, so the def is what says whether
/// `x` is positional, optional or keyword.
pub(super) fn def_parameters(
    parameters: &ruby_prism::ParametersNode<'_>,
) -> Option<Vec<(String, ParamKind)>> {
    let mut out = Vec::new();
    for required in parameters.requireds().iter() {
        let node = required.as_required_parameter_node()?;
        out.push((constant_id_str(&node.name()).to_string(), ParamKind::Required));
    }
    for optional in parameters.optionals().iter() {
        let node = optional.as_optional_parameter_node()?;
        out.push((constant_id_str(&node.name()).to_string(), ParamKind::Optional));
    }
    if let Some(rest) = parameters.rest() {
        let node = rest.as_rest_parameter_node()?;
        let name = node.name()?;
        out.push((constant_id_str(&name).to_string(), ParamKind::Rest));
    }
    for keyword in parameters.keywords().iter() {
        if let Some(node) = keyword.as_required_keyword_parameter_node() {
            out.push((
                constant_id_str(&node.name()).to_string(),
                ParamKind::Keyword { required: true },
            ));
            continue;
        }
        let node = keyword.as_optional_keyword_parameter_node()?;
        out.push((
            constant_id_str(&node.name()).to_string(),
            ParamKind::Keyword { required: false },
        ));
    }
    if let Some(rest) = parameters.keyword_rest() {
        let node = rest.as_keyword_rest_parameter_node()?;
        let name = node.name()?;
        out.push((constant_id_str(&name).to_string(), ParamKind::KeywordRest));
    }
    if let Some(block) = parameters.block() {
        let name = block.name()?;
        out.push((constant_id_str(&name).to_string(), ParamKind::Block));
    }
    Some(out)
}

/// The sorbet type grammar this module reads. Anything else — generics
/// via `type_parameters`, `T.attached_class`, `T.self_type`, shapes,
/// proc types — returns `None`, which drops the whole signature.
/// The `Name = T.type_alias { … }` constants a class body declares,
/// on top of what the enclosing scopes already named.
///
/// A type alias is the one constant a `sig` can name that is NOT a
/// class, and the reader had no way to tell the difference — so a
/// parameter declared with one typed as a class nobody defines. The
/// emit already drops these constants for having no runtime; this is
/// the other half, reading what they were for.
pub(super) fn collect_type_aliases(
    statements: &[Node<'_>],
    outer: &HashMap<String, Ty>,
) -> HashMap<String, Ty> {
    let mut out = outer.clone();
    for statement in statements {
        let Some(write) = statement.as_constant_write_node() else { continue };
        let Some(call) = write.value().as_call_node() else { continue };
        if constant_id_str(&call.name()) != "type_alias" {
            continue;
        }
        let names_t = call
            .receiver()
            .and_then(|r| r.as_constant_read_node())
            .is_some_and(|c| constant_id_str(&c.name()) == "T");
        if !names_t {
            continue;
        }
        // `T.type_alias { X }` / `T.type_alias do X end` — the block's
        // single statement IS the type.
        let Some(body) = call
            .block()
            .and_then(|b| b.as_block_node())
            .and_then(|b| b.body())
            .and_then(|b| b.as_statements_node())
        else {
            continue;
        };
        let Some(expr) = body.body().iter().next() else { continue };
        // Resolved against what is known SO FAR, so an alias built
        // from an earlier one reads; a forward reference does not,
        // which is the same order Ruby itself requires.
        if let Some(ty) = sorbet_ty(&expr, true, &out) {
            out.insert(constant_id_str(&write.name()).to_string(), ty);
        }
    }
    out
}

/// `self_is_instance` mirrors the RBS reader's rule for `self`: on an
/// instance-side member `T.self_type` is an instance of the receiving
/// class; on a singleton member it is the class object, which `Ty`
/// cannot spell, so it stays unread. `T.attached_class` needs no such
/// test — it MEANS "an instance of the attached class" and sorbet only
/// admits it where that is what it is.
pub(super) fn sorbet_ty(
    node: &Node<'_>,
    self_is_instance: bool,
    aliases: &HashMap<String, Ty>,
) -> Option<Ty> {
    if let Some(read) = node.as_constant_read_node() {
        let name = constant_id_str(&read.name());
        if let Some(ty) = aliases.get(name) {
            return Some(ty.clone());
        }
        return Some(named_ty(name));
    }
    if let Some(path) = node.as_constant_path_node() {
        let name = constant_path_name(node);
        // `T::Boolean` is a type of its own; every other `T::…`
        // constant in a signature position is a container handled
        // below or something this grammar does not read.
        if name == "T::Boolean" {
            return Some(Ty::Bool);
        }
        if path.parent().is_some() {
            return Some(named_ty(&name));
        }
        return Some(named_ty(&name));
    }
    // `{ max_per_pod: Integer, in: Duration }` — a shape, the hash
    // whose keys are known. The same thing RBS spells `{ key: T }`.
    if let Some(hash) = node.as_hash_node() {
        let mut fields = indexmap::IndexMap::new();
        for element in hash.elements().iter() {
            let assoc = element.as_assoc_node()?;
            let key = assoc.key();
            let key = key.as_symbol_node()?;
            let name = String::from_utf8_lossy(key.value_loc()?.as_slice()).into_owned();
            fields.insert(
                Symbol::new(&name),
                sorbet_ty(&assoc.value(), self_is_instance, aliases)?,
            );
        }
        return Some(Ty::Record { row: crate::ty::Row { fields: fields.into(), rest: None } });
    }
    // `[Integer, String]` — a tuple.
    if let Some(array) = node.as_array_node() {
        let elems: Vec<Ty> = array
            .elements()
            .iter()
            .map(|e| sorbet_ty(&e, self_is_instance, aliases))
            .collect::<Option<_>>()?;
        return Some(Ty::Tuple { elems: elems.into() });
    }
    // `(Foo)` around a type is the type.
    if let Some(parens) = node.as_parentheses_node() {
        let body = parens.body()?.as_statements_node()?;
        let mut statements = body.body().iter();
        let only = statements.next()?;
        return if statements.next().is_none() {
            sorbet_ty(&only, self_is_instance, aliases)
        } else {
            None
        };
    }
    // `T::Array[String]`, `T::Hash[Symbol, Integer]`, `T::Set[X]`
    if let Some(index) = node.as_call_node() {
        let name = index.name();
        let method = constant_id_str(&name).to_string();
        // `T.proc.params(a: A).returns(R)` / `.void` — a block or
        // callable, the type RBS spells `^(A) -> R`. `bind(Foo)` says
        // what `self` is inside it and `checked` how hard runtime
        // checks it; neither is a type.
        if let Some(fn_ty) = sorbet_proc_ty(&index, self_is_instance, aliases) {
            return fn_ty;
        }
        if method == "[]" {
            let receiver = index.receiver()?;
            let container = constant_path_name(&receiver);
            // A generic app or gem class: `Result[Ok, Err]`
            // is an instance of that class whatever its parameters; a
            // parameter this grammar cannot read is `untyped`, not a
            // reason to lose the class.
            if !container.is_empty() && !container.starts_with("T::") {
                let args = index
                    .arguments()?
                    .arguments()
                    .iter()
                    .map(|a| sorbet_ty(&a, self_is_instance, aliases).unwrap_or(Ty::Untyped))
                    .collect();
                return Some(Ty::Class { id: ClassId(Symbol::new(&container)), args });
            }
            let args: Vec<Ty> = index
                .arguments()?
                .arguments()
                .iter()
                .map(|a| sorbet_ty(&a, self_is_instance, aliases))
                .collect::<Option<_>>()?;
            return crate::rbs::sorbet_generic_ty(&container, &args);
        }
        // `T.nilable(X)`, `T.any(A, B)`, `T.untyped`
        let receiver = index.receiver()?;
        if constant_id_str(&receiver.as_constant_read_node()?.name()) != "T" {
            return None;
        }
        return match method.as_str() {
            "untyped" => Some(Ty::Untyped),
            // The receiver-dependent type, and the reason this whole
            // seam exists: a factory declared once on a base class
            // answers with an instance of whichever subclass called
            // it. `dispatch` substitutes it there.
            "attached_class" => Some(Ty::SelfInstance),
            "self_type" if self_is_instance => Some(Ty::SelfInstance),
            // A class object. `Ty` has no class-object type — a
            // constant read types as the class itself and dispatch
            // consults both sides — so the class stands for it, named
            // as written (`T.class_of(String)` is the class String).
            "class_of" => {
                let argument = index.arguments()?.arguments().iter().next()?;
                let id = constant_path_name(&argument);
                (!id.is_empty()).then(|| Ty::Class { id: ClassId(Symbol::new(&id)), args: Vec::new().into() })
            }
            // What `raise` returns: no value.
            "noreturn" => Some(Ty::Bottom),
            // The supertype of everything; nothing can be called on it,
            // so `untyped` answers the same.
            "anything" => Some(Ty::Untyped),
            // `T.type_parameter(:U)`, declared by `type_parameters(:U)`.
            // The analyzer does not instantiate a signature's variables
            // per call: an unmodelled boundary, so `untyped`.
            "type_parameter" => Some(Ty::Untyped),
            "all" => {
                let members: Vec<Ty> = index
                    .arguments()?
                    .arguments()
                    .iter()
                    .map(|a| sorbet_ty(&a, self_is_instance, aliases))
                    .collect::<Option<_>>()?;
                (!members.is_empty()).then(|| crate::rbs::intersection_ty(members))
            }
            "nilable" => {
                let inner =
                    sorbet_ty(&index.arguments()?.arguments().iter().next()?, self_is_instance, aliases)?;
                Some(Ty::Union { variants: vec![inner, Ty::Nil].into() })
            }
            "any" => {
                let variants: Vec<Ty> = index
                    .arguments()?
                    .arguments()
                    .iter()
                    .map(|a| sorbet_ty(&a, self_is_instance, aliases))
                    .collect::<Option<_>>()?;
                (variants.len() > 1).then_some(Ty::Union { variants: variants.into() })
            }
            _ => None,
        };
    }
    None
}

/// A `T.proc` chain read as `Ty::Fn`. `Some(None)` when the chain is a
/// `T.proc` this grammar cannot read (the caller drops the signature),
/// `None` when the call is not a `T.proc` chain at all.
fn sorbet_proc_ty(
    call: &ruby_prism::CallNode<'_>,
    self_is_instance: bool,
    aliases: &HashMap<String, Ty>,
) -> Option<Option<Ty>> {
    // Walk to the root first: only `T.proc` heads such a chain.
    let mut chain = vec![call.as_node().as_call_node()?];
    loop {
        let receiver = chain.last()?.receiver()?;
        match receiver.as_call_node() {
            Some(next) => chain.push(next),
            None => {
                let root = chain.pop()?;
                let names_t = receiver
                    .as_constant_read_node()
                    .is_some_and(|c| constant_id_str(&c.name()) == "T");
                if !names_t || constant_id_str(&root.name()) != "proc" {
                    return None;
                }
                break;
            }
        }
    }
    let read = || -> Option<Ty> {
        let mut params = Vec::new();
        let mut ret: Option<Ty> = None;
        // Outermost link first; the inner `params` is written first, so
        // walk from the root outward.
        for link in chain.iter().rev() {
            match constant_id_str(&link.name()) {
                "params" => {
                    for argument in link.arguments()?.arguments().iter() {
                        let hash = argument.as_keyword_hash_node()?;
                        for element in hash.elements().iter() {
                            let assoc = element.as_assoc_node()?;
                            let key = assoc.key();
                            let key = key.as_symbol_node()?;
                            let name =
                                String::from_utf8_lossy(key.value_loc()?.as_slice()).into_owned();
                            params.push(Param {
                                name: Symbol::new(&name),
                                ty: sorbet_ty(&assoc.value(), self_is_instance, aliases)?.into(),
                                kind: ParamKind::Required,
                            });
                        }
                    }
                }
                "returns" => {
                    let argument = link.arguments()?.arguments().iter().next()?;
                    ret = Some(sorbet_ty(&argument, self_is_instance, aliases)?);
                }
                "void" => ret = Some(Ty::Nil),
                "bind" | "checked" => {}
                _ => return None,
            }
        }
        Some(Ty::Fn {
            params: params.into(),
            block: None,
            ret: std::sync::Arc::new(ret.unwrap_or(Ty::Untyped)),
            effects: EffectSet::default(),
        })
    };
    Some(read())
}

/// A constant in type position, mapped the way the RBS reader maps the
/// same names so both sources agree.
fn named_ty(name: &str) -> Ty {
    match name {
        "Integer" => Ty::Int,
        "Float" => Ty::Float,
        "String" => Ty::Str,
        "Hash" => Ty::Hash { key: std::sync::Arc::new(Ty::Untyped), value: std::sync::Arc::new(Ty::Untyped) },
        "Array" => Ty::Array { elem: std::sync::Arc::new(Ty::Untyped) },
        "Symbol" => Ty::Sym,
        "TrueClass" | "FalseClass" => Ty::Bool,
        "NilClass" => Ty::Nil,
        "Date" => Ty::Date,
        "Time" | "DateTime" | "ActiveSupport::TimeWithZone" => Ty::Time,
        _ => Ty::Class { id: ClassId(Symbol::new(name)), args: Vec::new().into() },
    }
}

/// The `Ty::Fn` an RBS inline comment declares for a `def`:
///
/// ```ruby
/// #: (::Shop, ActionDispatch::Request) -> void
/// def initialize(shop, request)
/// ```
///
/// The comment lines are the contiguous run directly above the `def`
/// (other `#` comments may sit among them: `# @override`). `#:` starts
/// the signature and `#|` continues it. RBS leaves positional
/// parameters unnamed; the def names them, so the two are paired in
/// order and anything that does not pair up (a different count or
/// kind, a keyword the def does not have) drops the signature rather
/// than mistyping a parameter.
fn rbs_comment_signature(
    source: &[u8],
    def: &ruby_prism::DefNode<'_>,
    in_singleton_class: bool,
    rbs_aliases: &crate::rbs::AliasTable,
) -> Option<Ty> {
    let text = rbs_comment_text(source, def.location().start_offset())?;
    // A `#: type name = ...` comment is a declaration, not a signature.
    if text.starts_with("type ") {
        return None;
    }
    let receiver = if in_singleton_class || def.receiver().is_some() { "self." } else { "" };
    let wrapped = format!("class X\n  def {receiver}m: {text}\nend\n");
    let sigs = crate::rbs::parse_signatures_with_aliases(&wrapped, rbs_aliases).ok()?;
    let Ty::Fn { params: declared, block, ret, effects } = sigs.methods.into_iter().next()?.1 else {
        return None;
    };
    let (declared_block, declared): (Vec<Param>, Vec<Param>) =
        declared.into_iter().partition(|p| matches!(p.kind, ParamKind::Block));
    let def_params = match def.parameters() {
        Some(parameters) => {
            if parameters.posts().iter().next().is_some() {
                return None;
            }
            def_parameters(&parameters)?
        }
        None => Vec::new(),
    };
    let def_block = def_params.iter().find(|(_, k)| matches!(k, ParamKind::Block));
    let def_params: Vec<&(String, ParamKind)> =
        def_params.iter().filter(|(_, k)| !matches!(k, ParamKind::Block)).collect();
    if def_params.len() != declared.len() {
        return None;
    }
    let mut declared: Vec<Option<Param>> = declared.into_iter().map(Some).collect();
    let mut next_positional = 0;
    let mut params = Vec::new();
    for (name, kind) in def_params {
        let index = if matches!(kind, ParamKind::Keyword { .. }) {
            declared.iter().position(|candidate| candidate.as_ref().is_some_and(|param| {
                matches!(param.kind, ParamKind::Keyword { .. }) && param.name.as_str() == name
            }))?
        } else {
            let index = (next_positional..declared.len()).find(|&index| {
                declared[index].as_ref().is_some_and(|param| !matches!(param.kind, ParamKind::Keyword { .. }))
            })?;
            next_positional = index + 1;
            index
        };
        let decl = declared[index].take()?;
        if *kind != decl.kind {
            return None;
        }
        params.push(Param { name: Symbol::new(name), ty: decl.ty, kind: kind.clone() });
    }
    if declared.into_iter().any(|param| param.is_some()) {
        return None;
    }
    if let Some((name, _)) = def_block {
        let ty = declared_block.into_iter().next().map(|p| p.ty).unwrap_or(Ty::Untyped.into());
        params.push(Param { name: Symbol::new(name), ty, kind: ParamKind::Block });
    }
    Some(Ty::Fn { params: params.into(), block, ret, effects })
}

/// The `#: type name = ...` aliases a class or module body declares,
/// layered over the enclosing scope's (`outer`); a name declared again
/// shadows the outer one.
///
/// Sorbet's RBS comments spell a type alias as a comment in the body
/// (`#: type object_type = ::User`, continued with `#|`) and refer to
/// it by name from any signature below. Resolve each scope before walking
/// nested bodies so an inherited alias keeps its original dependencies
/// even when a child shadows one. Nested class and module bodies are skipped: an alias
/// declared there is not visible here.
fn scoped_rbs_aliases(
    source: &[u8],
    body: ruby_prism::Location<'_>,
    statements: &[Node<'_>],
    outer: &crate::rbs::AliasTable,
) -> crate::rbs::AliasTable {
    let mut out = outer.clone();
    let Ok(text) = std::str::from_utf8(source) else { return out };
    let nested: Vec<(usize, usize)> = statements
        .iter()
        .filter_map(|s| {
            let location = s.as_class_node().map(|c| c.location()).or_else(|| s.as_module_node().map(|m| m.location()))?;
            Some((location.start_offset(), location.end_offset()))
        })
        .collect();
    let mut offset = body.start_offset();
    let Some(region) = text.get(body.start_offset()..body.end_offset()) else { return out };
    let mut current: Option<String> = None;
    let mut declarations: Vec<String> = Vec::new();
    for line in region.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if nested.iter().any(|(s, e)| start >= *s && start < *e) {
            declarations.extend(current.take());
            continue;
        }
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#:").map(str::trim).filter(|r| r.starts_with("type ")) {
            declarations.extend(current.take());
            current = Some(rest.to_string());
        } else if let (Some(rest), Some(open)) = (trimmed.strip_prefix("#|"), current.as_mut()) {
            open.push(' ');
            open.push_str(rest.trim());
        } else {
            declarations.extend(current.take());
        }
    }
    declarations.extend(current);
    let mut local: Vec<(String, String)> = Vec::new();
    for declaration in declarations {
        let name: String = declaration["type ".len()..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        out.remove(&name);
        local.retain(|(existing, _)| *existing != name);
        local.push((name, declaration));
    }
    // Parse independently: a malformed unused declaration must not discard
    // valid aliases or signatures elsewhere in the same body.
    let parsed: Vec<_> = local.iter()
        .filter_map(|(_, declaration)| ruby_rbs::node::parse(declaration).ok())
        .collect();
    let members: Vec<_> = parsed.iter().flat_map(|s| s.declarations().iter()).collect();
    crate::rbs::resolve_aliases(&members, None, &out)
}

/// The signature text of the `#:` / `#|` comment lines directly above
/// the line holding byte `def_start`.
fn rbs_comment_text(source: &[u8], def_start: usize) -> Option<String> {
    let text = std::str::from_utf8(source).ok()?;
    let before = text.get(..def_start)?;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let mut lines: Vec<&str> = Vec::new();
    for line in text[..line_start].lines().rev() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            break;
        }
        lines.push(trimmed);
    }
    lines.reverse();
    let mut sig = String::new();
    let mut open = false;
    for line in lines {
        if let Some(rest) = line.strip_prefix("#:") {
            // A second `#:` opens another declaration; only the last
            // block counts, matching what sits nearest the def.
            sig.clear();
            sig.push_str(rest.trim());
            open = true;
        } else if let Some(rest) = line.strip_prefix("#|") {
            if open {
                sig.push(' ');
                sig.push_str(rest.trim());
            }
        } else {
            open = false;
        }
    }
    (!sig.is_empty()).then_some(sig)
}

/// A Sorbet type expression (`T.nilable(Foo)`, `T::Array[String]`, ...)
/// read as a `Ty`, for `T.let(x, Type)`.
pub(super) fn sorbet_type_node(node: &Node<'_>) -> Option<Ty> {
    sorbet_ty(node, true, &HashMap::new())
}
