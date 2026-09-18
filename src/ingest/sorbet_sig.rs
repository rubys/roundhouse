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
    let result = ruby_prism::parse(source);
    let node = result.node();
    let Some(program) = node.as_program_node() else {
        return HashMap::new();
    };
    let mut out: HashMap<ClassId, HashMap<Symbol, Ty>> = HashMap::new();
    walk(&program.statements().body().iter().collect::<Vec<_>>(), None, &mut out);
    out
}

fn walk(
    statements: &[Node<'_>],
    scope: Option<&str>,
    out: &mut HashMap<ClassId, HashMap<Symbol, Ty>>,
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
                    walk(&body.body().iter().collect::<Vec<_>>(), Some(&name), out);
                }
            }
            pending = None;
            continue;
        }
        if let Some(module) = statement.as_module_node() {
            let name = qualify(scope, &constant_path_name(&module.constant_path()));
            if let Some(body) = module.body() {
                if let Some(body) = body.as_statements_node() {
                    walk(&body.body().iter().collect::<Vec<_>>(), Some(&name), out);
                }
            }
            pending = None;
            continue;
        }
        if let Some(def) = statement.as_def_node() {
            if let (Some(sig), Some(scope)) = (pending.take(), scope) {
                if let Some(ty) = signature_ty(&statements[sig], &def) {
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

fn is_sig_call(call: &ruby_prism::CallNode<'_>) -> bool {
    let name = call.name();
    constant_id_str(&name) == "sig" && call.receiver().is_none() && call.block().is_some()
}

/// `Foo`, `Foo::Bar` — the class's own path as written.
fn constant_path_name(node: &Node<'_>) -> String {
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

fn qualify(scope: Option<&str>, name: &str) -> String {
    match scope {
        Some(scope) if !name.is_empty() => format!("{scope}::{name}"),
        _ => name.to_string(),
    }
}

/// The `Ty::Fn` a `sig { … }` declares for the `def` below it, or
/// `None` when any part of it is outside the grammar.
fn signature_ty(sig: &Node<'_>, def: &ruby_prism::DefNode<'_>) -> Option<Ty> {
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
                    returns = Some(sorbet_ty(&argument)?);
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
                        declared.insert(name, sorbet_ty(&assoc.value())?);
                    }
                }
            }
            // Modifiers carry no type: `override`, `overridable`,
            // `abstract`, `final`, `checked(:never)`, `type_parameters`.
            "override" | "overridable" | "abstract" | "final" | "checked" => {}
            _ => return None,
        }
        link = call.receiver();
    }
    if !saw_return_clause {
        return None;
    }

    let mut params = Vec::new();
    if let Some(parameters) = def.parameters() {
        for (name, kind) in def_parameters(&parameters)? {
            let ty = declared.remove(&name)?;
            params.push(Param { name: Symbol::new(&name), ty, kind });
        }
    }
    // A name in the sig that the def does not have means the pairing is
    // wrong, not that the extra entry is harmless.
    if !declared.is_empty() {
        return None;
    }

    Some(Ty::Fn {
        params,
        block: None,
        ret: Box::new(returns?),
        effects: EffectSet::pure(),
    })
}

/// The def's own parameters, in order, with the kind Ruby gives them —
/// the sig names them all the same way, so the def is what says whether
/// `x` is positional, optional or keyword.
fn def_parameters(
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
fn sorbet_ty(node: &Node<'_>) -> Option<Ty> {
    if let Some(read) = node.as_constant_read_node() {
        return Some(named_ty(constant_id_str(&read.name())));
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
    // `T::Array[String]`, `T::Hash[Symbol, Integer]`, `T::Set[X]`
    if let Some(index) = node.as_call_node() {
        let name = index.name();
        let method = constant_id_str(&name).to_string();
        if method == "[]" {
            let receiver = index.receiver()?;
            let container = constant_path_name(&receiver);
            let args: Vec<Ty> = index
                .arguments()?
                .arguments()
                .iter()
                .map(|a| sorbet_ty(&a))
                .collect::<Option<_>>()?;
            return match (container.as_str(), args.as_slice()) {
                ("T::Array", [elem]) => Some(Ty::Array { elem: Box::new(elem.clone()) }),
                ("T::Hash", [key, value]) => Some(Ty::Hash {
                    key: Box::new(key.clone()),
                    value: Box::new(value.clone()),
                }),
                _ => None,
            };
        }
        // `T.nilable(X)`, `T.any(A, B)`, `T.untyped`
        let receiver = index.receiver()?;
        if constant_id_str(&receiver.as_constant_read_node()?.name()) != "T" {
            return None;
        }
        return match method.as_str() {
            "untyped" => Some(Ty::Untyped),
            "nilable" => {
                let inner = sorbet_ty(&index.arguments()?.arguments().iter().next()?)?;
                Some(Ty::Union { variants: vec![inner, Ty::Nil] })
            }
            "any" => {
                let variants: Vec<Ty> = index
                    .arguments()?
                    .arguments()
                    .iter()
                    .map(|a| sorbet_ty(&a))
                    .collect::<Option<_>>()?;
                (variants.len() > 1).then_some(Ty::Union { variants })
            }
            _ => None,
        };
    }
    None
}

/// A constant in type position, mapped the way the RBS reader maps the
/// same names so both sources agree.
fn named_ty(name: &str) -> Ty {
    match name {
        "Integer" => Ty::Int,
        "Float" => Ty::Float,
        "String" => Ty::Str,
        "Symbol" => Ty::Sym,
        "TrueClass" | "FalseClass" => Ty::Bool,
        "NilClass" => Ty::Nil,
        "Time" | "Date" | "DateTime" | "ActiveSupport::TimeWithZone" => Ty::Time,
        _ => Ty::Class { id: ClassId(Symbol::new(name)), args: Vec::new() },
    }
}
