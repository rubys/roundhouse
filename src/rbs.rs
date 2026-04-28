//! RBS → Roundhouse `Ty` mapping.
//!
//! Parses RBS source (via `ruby-rbs`) and extracts method signatures as
//! `Ty::Fn` values, keyed by method name. The first consumer of this is
//! the runtime-extraction pipeline: a Ruby+RBS authored function becomes
//! typed IR that emitters can turn into target-language code.
//!
//! Scope for now: module/class bodies containing `def` methods with a
//! single overload, required positional parameters only, and a bounded
//! set of types (bases, Array, Hash, optional, union, user classes).
//! Keyword args, blocks, rest/splat, and multi-overloads are recognized
//! but rejected with `Err` rather than silently dropped.

use ruby_rbs::node::{Node, parse};

use crate::effect::EffectSet;
use crate::ident::{ClassId, Symbol};
use crate::ty::{Param, ParamKind, Ty};

/// Signatures extracted from an RBS source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Signatures {
    /// Method name → signature (`Ty::Fn`). Order matches RBS source order.
    pub methods: Vec<(Symbol, Ty)>,
}

/// Parse RBS source and extract method signatures.
pub fn parse_signatures(source: &str) -> Result<Signatures, String> {
    let signature = parse(source)?;
    let mut out = Signatures::default();

    for decl in signature.declarations().iter() {
        match decl {
            Node::Class(class) => collect_members(class.members().iter(), &mut out)?,
            Node::Module(module) => collect_members(module.members().iter(), &mut out)?,
            Node::Interface(iface) => collect_members(iface.members().iter(), &mut out)?,
            _ => {}
        }
    }

    Ok(out)
}

/// Parse RBS source and extract method signatures grouped by their
/// enclosing class/module. Used by Rails-app ingestion to apply
/// user-authored RBS sidecars — `sig/**/*.rbs` files that declare
/// method signatures for app classes the Rails conventions can't
/// fully type on their own (helper modules, concerns, service
/// classes, ad-hoc user methods).
///
/// Nested classes/modules produce namespaced `ClassId`s joined with
/// `::` (e.g., `Api::V1::Post`). Instance and singleton methods are
/// merged into the same method table today — the analyzer's dispatch
/// looks up in both class_methods and instance_methods anyway, so
/// the distinction can be recovered later when it matters.
pub fn parse_app_signatures(
    source: &str,
) -> Result<std::collections::HashMap<ClassId, std::collections::HashMap<Symbol, Ty>>, String> {
    let signature = parse(source)?;
    let mut out: std::collections::HashMap<ClassId, std::collections::HashMap<Symbol, Ty>> =
        std::collections::HashMap::new();

    for decl in signature.declarations().iter() {
        walk_decl(&decl, None, &mut out)?;
    }

    Ok(out)
}

fn walk_decl(
    decl: &Node<'_>,
    parent: Option<&str>,
    out: &mut std::collections::HashMap<ClassId, std::collections::HashMap<Symbol, Ty>>,
) -> Result<(), String> {
    match decl {
        Node::Class(class) => {
            let name = namespace_join(parent, class.name().name().as_str());
            collect_class_methods(class.members().iter(), &name, out)?;
        }
        Node::Module(module) => {
            let name = namespace_join(parent, module.name().name().as_str());
            collect_class_methods(module.members().iter(), &name, out)?;
        }
        Node::Interface(iface) => {
            let name = namespace_join(parent, iface.name().name().as_str());
            collect_class_methods(iface.members().iter(), &name, out)?;
        }
        _ => {}
    }
    Ok(())
}

fn collect_class_methods<'a, I: Iterator<Item = Node<'a>>>(
    members: I,
    class_name: &str,
    out: &mut std::collections::HashMap<ClassId, std::collections::HashMap<Symbol, Ty>>,
) -> Result<(), String> {
    let class_id = ClassId(Symbol::new(class_name));
    for member in members {
        match member {
            Node::MethodDefinition(method) => {
                let name = Symbol::new(method.name().as_str());
                let ty = method_signature_ty(&method)?;
                out.entry(class_id.clone()).or_default().insert(name, ty);
            }
            // Nested class/module inside this one — recurse with the
            // combined namespace.
            Node::Class(_) | Node::Module(_) | Node::Interface(_) => {
                walk_decl(&member, Some(class_name), out)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn namespace_join(parent: Option<&str>, name: &str) -> String {
    match parent {
        Some(p) => format!("{p}::{name}"),
        None => name.to_string(),
    }
}

fn collect_members<'a, I: Iterator<Item = Node<'a>>>(
    members: I,
    out: &mut Signatures,
) -> Result<(), String> {
    for member in members {
        match member {
            Node::MethodDefinition(method) => {
                let name = Symbol::new(method.name().as_str());
                let ty = method_signature_ty(&method)?;
                out.methods.push((name, ty));
            }
            // Nested class / module / interface — recurse so methods
            // defined inside get collected into the same flat table.
            // Mirrors `parse_methods` on the Ruby side, which walks
            // into class/module bodies to collect their `def`s.
            Node::Class(class) => {
                collect_members(class.members().iter(), out)?;
            }
            Node::Module(module) => {
                collect_members(module.members().iter(), out)?;
            }
            Node::Interface(iface) => {
                collect_members(iface.members().iter(), out)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn method_signature_ty(method: &ruby_rbs::node::MethodDefinitionNode<'_>) -> Result<Ty, String> {
    let mut overloads = method.overloads().iter();
    let first = overloads
        .next()
        .ok_or_else(|| format!("method `{}` has no overloads", method.name()))?;
    if overloads.next().is_some() {
        return Err(format!(
            "method `{}` has multiple overloads; not yet supported",
            method.name()
        ));
    }

    let Node::MethodDefinitionOverload(overload) = first else {
        return Err(format!(
            "method `{}` has an unexpected overload node",
            method.name()
        ));
    };

    let Node::MethodType(method_type) = overload.method_type() else {
        return Err(format!(
            "method `{}` overload's method_type is unexpected",
            method.name()
        ));
    };

    let Node::FunctionType(fn_type) = method_type.type_() else {
        return Err(format!(
            "method `{}` has an untyped or proc-typed function; not yet supported",
            method.name()
        ));
    };

    let mut params = Vec::new();
    let method_name = method.name().as_str().to_string();

    collect_function_params(
        fn_type.required_positionals().iter(),
        &mut params,
        ParamKind::Required,
        &method_name,
        "required",
    )?;
    collect_function_params(
        fn_type.optional_positionals().iter(),
        &mut params,
        ParamKind::Optional,
        &method_name,
        "optional",
    )?;

    // `*rest` positional. Prism-rbs models this as a single optional
    // FunctionParam; if present, it becomes one `Rest` param.
    if let Some(rest_node) = fn_type.rest_positionals() {
        let Node::FunctionParam(fn_param) = rest_node else {
            return Err(format!(
                "method `{method_name}` rest positional is not a FunctionParam"
            ));
        };
        let name = fn_param
            .name()
            .map(|s| Symbol::new(s.as_str()))
            .unwrap_or_else(|| Symbol::new("rest"));
        let ty = ty_from_node(&fn_param.type_())?;
        params.push(Param { name, ty, kind: ParamKind::Rest });
    }

    collect_function_params(
        fn_type.trailing_positionals().iter(),
        &mut params,
        ParamKind::Required,
        &method_name,
        "trailing",
    )?;

    // Required keywords: `k: Ty` (no default marker on the RBS side).
    for (key, value) in fn_type.required_keywords().iter() {
        let name = keyword_name(&key, &method_name, "required keyword")?;
        let Node::FunctionParam(fn_param) = value else {
            return Err(format!(
                "method `{method_name}` required keyword `{}` is not a FunctionParam",
                name.as_str()
            ));
        };
        let ty = ty_from_node(&fn_param.type_())?;
        params.push(Param {
            name,
            ty,
            kind: ParamKind::Keyword { required: true },
        });
    }

    // Optional keywords: `?k: Ty`.
    for (key, value) in fn_type.optional_keywords().iter() {
        let name = keyword_name(&key, &method_name, "optional keyword")?;
        let Node::FunctionParam(fn_param) = value else {
            return Err(format!(
                "method `{method_name}` optional keyword `{}` is not a FunctionParam",
                name.as_str()
            ));
        };
        let ty = ty_from_node(&fn_param.type_())?;
        params.push(Param {
            name,
            ty,
            kind: ParamKind::Keyword { required: false },
        });
    }

    // `**rest_keywords` / `**opts`.
    if let Some(rest_node) = fn_type.rest_keywords() {
        let Node::FunctionParam(fn_param) = rest_node else {
            return Err(format!(
                "method `{method_name}` rest keywords is not a FunctionParam"
            ));
        };
        let name = fn_param
            .name()
            .map(|s| Symbol::new(s.as_str()))
            .unwrap_or_else(|| Symbol::new("opts"));
        let ty = ty_from_node(&fn_param.type_())?;
        params.push(Param {
            name,
            ty,
            kind: ParamKind::KeywordRest,
        });
    }

    let ret = ty_from_node(&fn_type.return_type())?;

    // Block signature: `{ (...) -> T }` — captured on method_type, not
    // fn_type. For now we only surface its presence via the block
    // param in `Ty::Fn`; full block-signature typing is a future step.
    // Use `Ty::Untyped` (not `Var`) for the placeholder: it's an
    // author-signed declaration that the block exists with a yet-to-be-
    // typed signature, not an inference gap.
    let block = method_type.block().map(|_| Box::new(Ty::Untyped));
    if block.is_some() {
        params.push(Param {
            name: Symbol::new("block"),
            ty: Ty::Untyped,
            kind: ParamKind::Block,
        });
    }

    Ok(Ty::Fn {
        params,
        block,
        ret: Box::new(ret),
        effects: EffectSet::pure(),
    })
}

fn keyword_name(key: &Node<'_>, method_name: &str, category: &str) -> Result<Symbol, String> {
    if let Node::Symbol(sym) = key {
        Ok(Symbol::new(sym.as_str()))
    } else {
        Err(format!(
            "method `{method_name}` {category} key is not a symbol"
        ))
    }
}

fn collect_function_params<'a, I: Iterator<Item = Node<'a>>>(
    iter: I,
    out: &mut Vec<Param>,
    kind: ParamKind,
    method_name: &str,
    category: &str,
) -> Result<(), String> {
    // Placeholder prefix for unnamed positionals. Keep "arg" for the
    // required/optional/trailing cases so the existing convention
    // (and dependent tests) stays stable.
    for (idx, node) in iter.enumerate() {
        let Node::FunctionParam(fn_param) = node else {
            return Err(format!(
                "method `{method_name}` {category} #{idx} is not a FunctionParam"
            ));
        };
        let name = fn_param
            .name()
            .map(|s| Symbol::new(s.as_str()))
            .unwrap_or_else(|| Symbol::new(format!("arg{idx}")));
        let ty = ty_from_node(&fn_param.type_())?;
        out.push(Param { name, ty, kind: kind.clone() });
    }
    Ok(())
}

fn ty_from_node(node: &Node<'_>) -> Result<Ty, String> {
    match node {
        Node::ClassInstanceType(class_type) => {
            let name_sym = class_type.name().name();
            let name = name_sym.as_str();
            let args: Vec<Ty> = class_type
                .args()
                .iter()
                .map(|n| ty_from_node(&n))
                .collect::<Result<_, _>>()?;
            Ok(map_class_instance(name, args))
        }
        Node::BoolType(_) => Ok(Ty::Bool),
        Node::NilType(_) => Ok(Ty::Nil),
        Node::VoidType(_) => Ok(Ty::Nil),
        // `untyped` is RBS's gradual escape hatch — explicit
        // author-signed opt-out from checking. Maps to `Ty::Untyped`
        // (distinct from `Ty::Var`, which is the analyzer's "I don't
        // know yet" sentinel for inference gaps). Untyped propagates
        // through dispatch unconditionally; targets that admit a
        // gradual escape (TS `any`, Python `Any`) emit it cleanly,
        // strict targets (Rust, Go) elevate it to a Diagnostic::Error
        // at emit time.
        Node::AnyType(_) => Ok(Ty::Untyped),
        Node::OptionalType(opt) => {
            let inner = ty_from_node(&opt.type_())?;
            Ok(union_of(vec![inner, Ty::Nil]))
        }
        Node::UnionType(u) => {
            let variants: Vec<Ty> = u
                .types()
                .iter()
                .map(|n| ty_from_node(&n))
                .collect::<Result<_, _>>()?;
            Ok(union_of(variants))
        }
        Node::TupleType(t) => {
            let elems: Vec<Ty> = t
                .types()
                .iter()
                .map(|n| ty_from_node(&n))
                .collect::<Result<_, _>>()?;
            Ok(Ty::Tuple { elems })
        }
        other => Err(format!(
            "unsupported RBS type node: {}",
            type_node_kind(other)
        )),
    }
}

fn map_class_instance(name: &str, args: Vec<Ty>) -> Ty {
    match (name, args.as_slice()) {
        ("Integer", []) => Ty::Int,
        ("Float", []) => Ty::Float,
        ("String", []) => Ty::Str,
        ("Symbol", []) => Ty::Sym,
        ("TrueClass" | "FalseClass", []) => Ty::Bool,
        ("NilClass", []) => Ty::Nil,
        ("Array", [elem]) => Ty::Array {
            elem: Box::new(elem.clone()),
        },
        ("Hash", [key, value]) => Ty::Hash {
            key: Box::new(key.clone()),
            value: Box::new(value.clone()),
        },
        _ => Ty::Class {
            id: ClassId(Symbol::new(name)),
            args,
        },
    }
}

fn union_of(variants: Vec<Ty>) -> Ty {
    if variants.len() == 1 {
        variants.into_iter().next().unwrap()
    } else {
        Ty::Union { variants }
    }
}

fn type_node_kind(node: &Node<'_>) -> &'static str {
    match node {
        Node::ClassInstanceType(_) => "ClassInstanceType",
        Node::ClassSingletonType(_) => "ClassSingletonType",
        Node::InterfaceType(_) => "InterfaceType",
        Node::AliasType(_) => "AliasType",
        Node::LiteralType(_) => "LiteralType",
        Node::BoolType(_) => "BoolType",
        Node::NilType(_) => "NilType",
        Node::VoidType(_) => "VoidType",
        Node::AnyType(_) => "AnyType",
        Node::TopType(_) => "TopType",
        Node::BottomType(_) => "BottomType",
        Node::OptionalType(_) => "OptionalType",
        Node::UnionType(_) => "UnionType",
        Node::IntersectionType(_) => "IntersectionType",
        Node::TupleType(_) => "TupleType",
        Node::RecordType(_) => "RecordType",
        Node::ProcType(_) => "ProcType",
        Node::VariableType(_) => "VariableType",
        _ => "non-type node",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(src: &str) -> Ty {
        let sigs = parse_signatures(src).expect("parses");
        assert_eq!(sigs.methods.len(), 1, "expected exactly one method");
        sigs.methods.into_iter().next().unwrap().1
    }

    fn fn_parts(ty: Ty) -> (Vec<Param>, Ty) {
        if let Ty::Fn { params, ret, .. } = ty {
            (params, *ret)
        } else {
            panic!("expected Ty::Fn, got {ty:?}");
        }
    }

    #[test]
    fn pluralize_signature() {
        let src = "module Inflector\n  def pluralize: (Integer, String) -> String\nend\n";
        let (params, ret) = fn_parts(parse_one(src));
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].ty, Ty::Int);
        assert_eq!(params[0].kind, ParamKind::Required);
        assert_eq!(params[1].ty, Ty::Str);
        assert_eq!(ret, Ty::Str);
    }

    #[test]
    fn parameter_names_preserved_when_present() {
        let src = "module M\n  def f: (Integer count, String word) -> String\nend\n";
        let (params, _) = fn_parts(parse_one(src));
        assert_eq!(params[0].name.as_str(), "count");
        assert_eq!(params[1].name.as_str(), "word");
    }

    #[test]
    fn unnamed_parameters_get_positional_placeholders() {
        let src = "module M\n  def f: (Integer, String) -> String\nend\n";
        let (params, _) = fn_parts(parse_one(src));
        assert_eq!(params[0].name.as_str(), "arg0");
        assert_eq!(params[1].name.as_str(), "arg1");
    }

    #[test]
    fn base_types() {
        let src = "module M\n  def f: (Integer, Float, String, Symbol, bool, nil) -> void\nend\n";
        let (params, ret) = fn_parts(parse_one(src));
        assert_eq!(
            params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>(),
            vec![Ty::Int, Ty::Float, Ty::Str, Ty::Sym, Ty::Bool, Ty::Nil],
        );
        assert_eq!(ret, Ty::Nil);
    }

    #[test]
    fn array_and_hash() {
        let src = "module M\n  def f: (Array[Integer], Hash[String, Integer]) -> void\nend\n";
        let (params, _) = fn_parts(parse_one(src));
        assert_eq!(
            params[0].ty,
            Ty::Array {
                elem: Box::new(Ty::Int)
            }
        );
        assert_eq!(
            params[1].ty,
            Ty::Hash {
                key: Box::new(Ty::Str),
                value: Box::new(Ty::Int),
            }
        );
    }

    #[test]
    fn optional_maps_to_union_with_nil() {
        let src = "module M\n  def f: (String?) -> void\nend\n";
        let (params, _) = fn_parts(parse_one(src));
        assert_eq!(
            params[0].ty,
            Ty::Union {
                variants: vec![Ty::Str, Ty::Nil]
            }
        );
    }

    #[test]
    fn union_types() {
        let src = "module M\n  def f: (Integer | String) -> void\nend\n";
        let (params, _) = fn_parts(parse_one(src));
        assert_eq!(
            params[0].ty,
            Ty::Union {
                variants: vec![Ty::Int, Ty::Str]
            }
        );
    }

    #[test]
    fn tuple_types() {
        let src = "module M\n  def f: ([Integer, String]) -> void\nend\n";
        let (params, _) = fn_parts(parse_one(src));
        assert_eq!(
            params[0].ty,
            Ty::Tuple {
                elems: vec![Ty::Int, Ty::Str]
            }
        );
    }

    #[test]
    fn user_class_becomes_class_id() {
        let src = "module M\n  def f: (Article) -> void\nend\n";
        let (params, _) = fn_parts(parse_one(src));
        let Ty::Class { id, args } = &params[0].ty else {
            panic!("expected Class, got {:?}", params[0].ty);
        };
        assert_eq!(id.0.as_str(), "Article");
        assert!(args.is_empty());
    }

    #[test]
    fn generic_user_class_keeps_args() {
        let src = "module M\n  def f: (Relation[Article]) -> void\nend\n";
        let (params, _) = fn_parts(parse_one(src));
        let Ty::Class { id, args } = &params[0].ty else {
            panic!("expected Class");
        };
        assert_eq!(id.0.as_str(), "Relation");
        assert_eq!(args.len(), 1);
        assert!(matches!(&args[0], Ty::Class { id, .. } if id.0.as_str() == "Article"));
    }

    #[test]
    fn multiple_methods_preserved_in_order() {
        let src = "module M\n  def a: () -> Integer\n  def b: () -> String\nend\n";
        let sigs = parse_signatures(src).expect("parses");
        assert_eq!(
            sigs.methods
                .iter()
                .map(|(n, _)| n.as_str().to_string())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn empty_return_voids_to_nil() {
        let src = "module M\n  def f: () -> void\nend\n";
        let (params, ret) = fn_parts(parse_one(src));
        assert!(params.is_empty());
        assert_eq!(ret, Ty::Nil);
    }

    #[test]
    fn effects_default_to_pure() {
        let src = "module M\n  def f: () -> void\nend\n";
        let Ty::Fn { effects, .. } = parse_one(src) else {
            panic!("expected Ty::Fn");
        };
        assert!(effects.is_pure());
    }

    #[test]
    fn multiple_overloads_are_rejected() {
        let src = "module M\n  def f: (Integer) -> Integer\n       | (String) -> String\nend\n";
        let err = parse_signatures(src).unwrap_err();
        assert!(
            err.contains("multiple overloads"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_errors_surface() {
        let err = parse_signatures("class { end").unwrap_err();
        assert!(!err.is_empty());
    }

    // ── parse_app_signatures ────────────────────────────────────────

    #[test]
    fn app_sigs_group_methods_by_class() {
        let src = "\
class Article
  def full_name: () -> String
  def word_count: () -> Integer
end

class Post
  def title: () -> String
end
";
        let out = parse_app_signatures(src).expect("parses");
        let article_id = ClassId(Symbol::from("Article"));
        let post_id = ClassId(Symbol::from("Post"));

        assert_eq!(out.len(), 2);
        let article_methods = &out[&article_id];
        assert_eq!(article_methods.len(), 2);
        assert!(article_methods.contains_key(&Symbol::from("full_name")));
        assert!(article_methods.contains_key(&Symbol::from("word_count")));

        let post_methods = &out[&post_id];
        assert_eq!(post_methods.len(), 1);
        assert!(post_methods.contains_key(&Symbol::from("title")));
    }

    #[test]
    fn app_sigs_namespace_nested_classes() {
        let src = "\
module Api
  class V1
    class Post
      def title: () -> String
    end
  end
end
";
        let out = parse_app_signatures(src).expect("parses");
        let nested = ClassId(Symbol::from("Api::V1::Post"));
        assert!(out.contains_key(&nested), "got keys: {:?}", out.keys().collect::<Vec<_>>());
        assert!(out[&nested].contains_key(&Symbol::from("title")));
    }

    #[test]
    fn app_sigs_module_methods() {
        let src = "\
module ApplicationHelper
  def format_date: (String) -> String
end
";
        let out = parse_app_signatures(src).expect("parses");
        let helper_id = ClassId(Symbol::from("ApplicationHelper"));
        assert!(out.contains_key(&helper_id));
        assert!(out[&helper_id].contains_key(&Symbol::from("format_date")));
    }

    #[test]
    fn app_sigs_method_signatures_are_ty_fn() {
        let src = "\
class Article
  def full_name: () -> String
end
";
        let out = parse_app_signatures(src).expect("parses");
        let methods = &out[&ClassId(Symbol::from("Article"))];
        let ty = &methods[&Symbol::from("full_name")];
        let Ty::Fn { ret, .. } = ty else {
            panic!("expected Ty::Fn, got {ty:?}");
        };
        assert_eq!(**ret, Ty::Str);
    }

    #[test]
    fn app_sigs_empty_source_is_empty_result() {
        let out = parse_app_signatures("").expect("parses");
        assert!(out.is_empty());
    }
}
