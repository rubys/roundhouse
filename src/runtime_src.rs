//! Parse a standalone Ruby source file (intended to hold runtime
//! library code authored in Ruby) into Roundhouse `MethodDef` values.
//!
//! This is the Ruby-body half of the runtime-extraction pipeline;
//! [`crate::rbs`] covers signatures. A later step marries the two: for
//! each method name, the body from here gets the signature from there.
//!
//! Scope: top-level `def`s and `def`s inside a single-level `module`/
//! `class` body. Required positional params only. Anything more exotic
//! (keyword args, rest/splat, blocks, nested scopes) is rejected with
//! `Err` rather than silently dropped, mirroring the RBS side.

use ruby_prism::{Node, parse};

use crate::dialect::{MethodDef, MethodReceiver};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ingest::ingest_expr;
use crate::rbs::parse_signatures;
use crate::span::Span;
use crate::ty::Ty;

const VIRTUAL_FILE: &str = "<runtime>";

/// Parse Ruby source and extract every `def` it finds (at top level
/// and one level inside module/class bodies) as a `MethodDef`.
pub fn parse_methods(source: &str) -> Result<Vec<MethodDef>, String> {
    let result = parse(source.as_bytes());

    let errors: Vec<String> = result
        .errors()
        .map(|e| e.message().to_string())
        .collect();
    if !errors.is_empty() {
        return Err(format!("parse error: {}", errors.join("; ")));
    }

    let root = result.node();
    let mut out = Vec::new();
    walk_scope(&root, &mut out)?;
    Ok(out)
}

/// Parse Ruby source and its RBS sidecar, returning `MethodDef`s with
/// the RBS-derived `Ty::Fn` attached to `signature`. Every Ruby method
/// must have a matching RBS signature and vice versa; arities must match.
/// Method-body expressions are left with `ty: None` — sub-expression
/// typing is a separate step.
pub fn parse_methods_with_rbs(
    ruby_src: &str,
    rbs_src: &str,
) -> Result<Vec<MethodDef>, String> {
    let mut methods = parse_methods(ruby_src)?;
    let sigs = parse_signatures(rbs_src)?;

    let mut sig_map: std::collections::HashMap<String, Ty> = sigs
        .methods
        .into_iter()
        .map(|(n, ty)| (n.as_str().to_string(), ty))
        .collect();

    for m in &mut methods {
        let ty = sig_map.remove(m.name.as_str()).ok_or_else(|| {
            format!("method `{}` has no matching RBS signature", m.name)
        })?;

        if let Ty::Fn { params, .. } = &ty {
            if params.len() != m.params.len() {
                return Err(format!(
                    "method `{}`: Ruby has {} positional param(s), RBS has {}",
                    m.name,
                    m.params.len(),
                    params.len()
                ));
            }
        } else {
            return Err(format!("method `{}`: signature is not Ty::Fn", m.name));
        }

        m.signature = Some(ty);
    }

    if !sig_map.is_empty() {
        let mut orphaned: Vec<String> = sig_map.keys().cloned().collect();
        orphaned.sort();
        return Err(format!(
            "RBS signature(s) with no matching Ruby method: {}",
            orphaned.join(", ")
        ));
    }

    // Run the body-typer on each method with the RBS-derived param
    // types seeded into the local environment. This populates `.ty`
    // throughout each body, which target emitters consume for
    // type-directed dispatch (Go's %d vs %s, eventually `==`
    // semantics per the type dispatch table).
    //
    // Runtime code doesn't reference user classes today, so the
    // dispatch table is empty — the body-typer falls back to its
    // primitive method tables for everything.
    let classes: std::collections::HashMap<crate::ident::ClassId, crate::analyze::ClassInfo> =
        std::collections::HashMap::new();
    let typer = crate::analyze::BodyTyper::new(&classes);
    for m in &mut methods {
        let mut ctx = crate::analyze::Ctx::default();
        if let Some(Ty::Fn { params, .. }) = &m.signature {
            for (name, p) in m.params.iter().zip(params.iter()) {
                ctx.local_bindings.insert(name.clone(), p.ty.clone());
            }
        }
        typer.analyze_expr(&mut m.body, &ctx);
    }

    Ok(methods)
}

fn walk_scope(node: &Node<'_>, out: &mut Vec<MethodDef>) -> Result<(), String> {
    if let Some(program) = node.as_program_node() {
        for stmt in program.statements().body().iter() {
            collect_from_stmt(&stmt, out)?;
        }
    } else if let Some(stmts) = node.as_statements_node() {
        for stmt in stmts.body().iter() {
            collect_from_stmt(&stmt, out)?;
        }
    }
    Ok(())
}

fn collect_from_stmt(node: &Node<'_>, out: &mut Vec<MethodDef>) -> Result<(), String> {
    if let Some(def) = node.as_def_node() {
        out.push(method_def_from(&def)?);
        return Ok(());
    }
    if let Some(module) = node.as_module_node() {
        if let Some(body) = module.body() {
            walk_scope(&body, out)?;
        }
        return Ok(());
    }
    if let Some(class) = node.as_class_node() {
        if let Some(body) = class.body() {
            walk_scope(&body, out)?;
        }
        return Ok(());
    }
    Ok(())
}

fn method_def_from(def: &ruby_prism::DefNode<'_>) -> Result<MethodDef, String> {
    let name_bytes = def.name().as_slice();
    let name = Symbol::new(
        std::str::from_utf8(name_bytes)
            .map_err(|_| "method name is not UTF-8".to_string())?,
    );

    let receiver = if def.receiver().is_some() {
        MethodReceiver::Class
    } else {
        MethodReceiver::Instance
    };

    let params = method_params(def, name.as_str())?;

    let body = match def.body() {
        Some(b) => ingest_expr(&b, VIRTUAL_FILE).map_err(|e| format!("in `{name}`: {e}"))?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(MethodDef {
        name,
        receiver,
        params,
        body,
        signature: None,
        effects: EffectSet::pure(),
    })
}

fn method_params(def: &ruby_prism::DefNode<'_>, method_name: &str) -> Result<Vec<Symbol>, String> {
    let Some(params_node) = def.parameters() else {
        return Ok(Vec::new());
    };

    // Anything beyond required positionals means the runtime source has
    // reached for a feature the extractor doesn't support yet. Better to
    // fail loudly than to silently drop a keyword arg or block param.
    if params_node.optionals().iter().next().is_some() {
        return Err(format!("method `{method_name}`: optional params not yet supported"));
    }
    if params_node.rest().is_some() {
        return Err(format!("method `{method_name}`: rest/splat params not yet supported"));
    }
    if params_node.keywords().iter().next().is_some() {
        return Err(format!("method `{method_name}`: keyword params not yet supported"));
    }
    if params_node.keyword_rest().is_some() {
        return Err(format!("method `{method_name}`: **kwargs not yet supported"));
    }
    if params_node.block().is_some() {
        return Err(format!("method `{method_name}`: block params not yet supported"));
    }
    if params_node.posts().iter().next().is_some() {
        return Err(format!(
            "method `{method_name}`: post-rest positional params not yet supported"
        ));
    }

    let mut names = Vec::new();
    for req in params_node.requireds().iter() {
        let rp = req.as_required_parameter_node().ok_or_else(|| {
            format!("method `{method_name}`: unexpected required-parameter shape")
        })?;
        let name_bytes = rp.name().as_slice();
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| format!("method `{method_name}`: param name is not UTF-8"))?;
        names.push(Symbol::new(name));
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprNode, InterpPart, Literal};

    fn parse_one(src: &str) -> MethodDef {
        let mut methods = parse_methods(src).expect("parses");
        assert_eq!(methods.len(), 1, "expected exactly one method");
        methods.remove(0)
    }

    #[test]
    fn toplevel_def_is_found() {
        let src = "def pluralize(count, word)\n  count == 1 ? \"1 #{word}\" : \"#{count} #{word}s\"\nend\n";
        let m = parse_one(src);
        assert_eq!(m.name.as_str(), "pluralize");
        assert_eq!(m.receiver, MethodReceiver::Instance);
        assert_eq!(
            m.params.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["count", "word"]
        );
    }

    #[test]
    fn module_nested_def_is_found() {
        let src = "module Inflector\n  def pluralize(count, word)\n    \"#{count} #{word}\"\n  end\nend\n";
        let m = parse_one(src);
        assert_eq!(m.name.as_str(), "pluralize");
        assert_eq!(m.params.len(), 2);
    }

    #[test]
    fn class_nested_def_is_found() {
        let src = "class Inflector\n  def f\n    1\n  end\nend\n";
        let m = parse_one(src);
        assert_eq!(m.name.as_str(), "f");
        assert!(m.params.is_empty());
    }

    #[test]
    fn self_receiver_is_class_kind() {
        let src = "module M\n  def self.f\n    1\n  end\nend\n";
        let m = parse_one(src);
        assert_eq!(m.receiver, MethodReceiver::Class);
    }

    #[test]
    fn pluralize_body_has_conditional_shape() {
        let src = "def pluralize(count, word)\n  count == 1 ? \"1 #{word}\" : \"#{count} #{word}s\"\nend\n";
        let m = parse_one(src);

        let (cond, then_branch, else_branch) = match *m.body.node {
            ExprNode::If {
                cond,
                then_branch,
                else_branch,
            } => (cond, then_branch, else_branch),
            other => panic!("expected If at body, got {other:?}"),
        };

        // cond: count == 1
        match *cond.node {
            ExprNode::Send { method, .. } => assert_eq!(method.as_str(), "=="),
            other => panic!("expected `==` send in cond, got {other:?}"),
        }

        // Then branch: "1 #{word}"
        match *then_branch.node {
            ExprNode::StringInterp { parts } => {
                assert!(has_literal_text(&parts, "1 "), "then-branch missing `1 `");
                assert!(has_expr_var(&parts, "word"), "then-branch missing `word`");
            }
            other => panic!("expected StringInterp in then-branch, got {other:?}"),
        }

        // Else branch: "#{count} #{word}s"
        match *else_branch.node {
            ExprNode::StringInterp { parts } => {
                assert!(has_expr_var(&parts, "count"), "else-branch missing `count`");
                assert!(has_expr_var(&parts, "word"), "else-branch missing `word`");
                assert!(has_literal_text(&parts, "s"), "else-branch missing trailing `s`");
            }
            other => panic!("expected StringInterp in else-branch, got {other:?}"),
        }
    }

    fn has_literal_text(parts: &[InterpPart], needle: &str) -> bool {
        parts.iter().any(|p| match p {
            InterpPart::Text { value } => value.contains(needle),
            _ => false,
        })
    }

    fn has_expr_var(parts: &[InterpPart], var: &str) -> bool {
        parts.iter().any(|p| match p {
            InterpPart::Expr { expr } => matches!(
                &*expr.node,
                ExprNode::Var { name, .. } if name.as_str() == var
            ),
            _ => false,
        })
    }

    #[test]
    fn multiple_defs_in_order() {
        let src = "def a; 1; end\ndef b; 2; end\n";
        let methods = parse_methods(src).expect("parses");
        assert_eq!(
            methods.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn integer_literal_body_roundtrips() {
        let src = "def f\n  42\nend\n";
        let m = parse_one(src);
        assert!(matches!(
            &*m.body.node,
            ExprNode::Lit {
                value: Literal::Int { value: 42 }
            }
        ));
    }

    #[test]
    fn multi_statement_body_is_sequenced() {
        let src = "def f\n  1\n  2\nend\n";
        let m = parse_one(src);
        let exprs = match *m.body.node {
            ExprNode::Seq { exprs } => exprs,
            other => panic!("expected Seq for multi-stmt body, got {other:?}"),
        };
        assert_eq!(exprs.len(), 2);
    }

    #[test]
    fn parse_error_surfaces() {
        let err = parse_methods("def f(").unwrap_err();
        assert!(err.contains("parse error"), "unexpected error: {err}");
    }

    #[test]
    fn keyword_params_rejected_explicitly() {
        let src = "def f(a:, b:)\n  1\nend\n";
        let err = parse_methods(src).unwrap_err();
        assert!(
            err.contains("keyword params"),
            "expected keyword-param rejection, got: {err}"
        );
    }

    #[test]
    fn splat_params_rejected_explicitly() {
        let src = "def f(*args)\n  1\nend\n";
        let err = parse_methods(src).unwrap_err();
        assert!(
            err.contains("rest/splat"),
            "expected splat rejection, got: {err}"
        );
    }

    #[test]
    fn block_params_rejected_explicitly() {
        let src = "def f(&blk)\n  1\nend\n";
        let err = parse_methods(src).unwrap_err();
        assert!(
            err.contains("block params"),
            "expected block-param rejection, got: {err}"
        );
    }

    #[test]
    fn optional_params_rejected_explicitly() {
        let src = "def f(a = 1)\n  a\nend\n";
        let err = parse_methods(src).unwrap_err();
        assert!(
            err.contains("optional params"),
            "expected optional-param rejection, got: {err}"
        );
    }

    #[test]
    fn def_without_params_or_body() {
        let src = "def f\nend\n";
        let m = parse_one(src);
        assert!(m.params.is_empty());
        assert!(matches!(&*m.body.node, ExprNode::Seq { exprs } if exprs.is_empty()));
    }

    // ── parse_methods_with_rbs ──────────────────────────────────────

    use crate::ty::{Param, ParamKind};

    const PLURALIZE_RB: &str =
        "module Inflector\n  def pluralize(count, word)\n    count == 1 ? \"1 #{word}\" : \"#{count} #{word}s\"\n  end\nend\n";
    const PLURALIZE_RBS: &str =
        "module Inflector\n  def pluralize: (Integer, String) -> String\nend\n";

    #[test]
    fn marrying_attaches_signature() {
        let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("types");
        assert_eq!(methods.len(), 1);
        let m = &methods[0];
        assert_eq!(m.name.as_str(), "pluralize");

        let sig = m.signature.as_ref().expect("signature attached");
        let Ty::Fn { params, ret, .. } = sig else {
            panic!("expected Ty::Fn, got {sig:?}");
        };
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].ty, Ty::Int);
        assert_eq!(params[1].ty, Ty::Str);
        assert_eq!(**ret, Ty::Str);

        // Param kinds come from RBS (Required in this case).
        assert!(params.iter().all(|p: &Param| p.kind == ParamKind::Required));
    }

    #[test]
    fn ruby_param_names_coexist_with_rbs_types() {
        // RBS has anonymous positionals; Ruby param names should survive.
        let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("types");
        let m = &methods[0];
        assert_eq!(
            m.params.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["count", "word"]
        );
    }

    #[test]
    fn ruby_method_missing_signature_errors() {
        let ruby = "def foo\n  1\nend\n";
        let rbs = "module M\nend\n";
        let err = parse_methods_with_rbs(ruby, rbs).unwrap_err();
        assert!(
            err.contains("foo") && err.contains("no matching RBS"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn orphan_rbs_signature_errors() {
        let ruby = "def foo\n  1\nend\n";
        let rbs = "module M\n  def foo: () -> Integer\n  def bar: () -> String\nend\n";
        let err = parse_methods_with_rbs(ruby, rbs).unwrap_err();
        assert!(
            err.contains("no matching Ruby method") && err.contains("bar"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn arity_mismatch_errors() {
        let ruby = "def f(a, b)\n  1\nend\n";
        let rbs = "module M\n  def f: (Integer) -> Integer\nend\n";
        let err = parse_methods_with_rbs(ruby, rbs).unwrap_err();
        assert!(
            err.contains("2 positional param") && err.contains("RBS has 1"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn multi_method_marrying_preserves_ruby_order() {
        let ruby = "module M\n  def b\n    1\n  end\n  def a\n    \"x\"\n  end\nend\n";
        let rbs = "module M\n  def a: () -> String\n  def b: () -> Integer\nend\n";
        let methods = parse_methods_with_rbs(ruby, rbs).expect("types");
        // Ruby order: b, a
        assert_eq!(
            methods.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        // And each has its own signature.
        let b_sig = methods[0].signature.as_ref().unwrap();
        let a_sig = methods[1].signature.as_ref().unwrap();
        assert!(matches!(b_sig, Ty::Fn { ret, .. } if **ret == Ty::Int));
        assert!(matches!(a_sig, Ty::Fn { ret, .. } if **ret == Ty::Str));
    }

    #[test]
    fn empty_ruby_and_empty_rbs_yields_empty() {
        let methods = parse_methods_with_rbs("", "").expect("types");
        assert!(methods.is_empty());
    }

    #[test]
    fn ruby_parse_error_surfaces_through_marrying() {
        let err = parse_methods_with_rbs("def f(", "module M\nend\n").unwrap_err();
        assert!(err.contains("parse error"), "unexpected: {err}");
    }

    #[test]
    fn rbs_parse_error_surfaces_through_marrying() {
        let err = parse_methods_with_rbs("", "class { end").unwrap_err();
        assert!(!err.is_empty());
    }

    // ── body-typer integration ──────────────────────────────────────

    fn find_var_ty(e: &crate::expr::Expr, name: &str) -> Option<Ty> {
        // Walk the tree looking for `Var { name }` and return its `.ty`.
        match &*e.node {
            ExprNode::Var { name: n, .. } if n.as_str() == name => e.ty.clone(),
            ExprNode::If { cond, then_branch, else_branch } => find_var_ty(cond, name)
                .or_else(|| find_var_ty(then_branch, name))
                .or_else(|| find_var_ty(else_branch, name)),
            ExprNode::Send { recv, args, .. } => {
                if let Some(r) = recv {
                    if let Some(t) = find_var_ty(r, name) {
                        return Some(t);
                    }
                }
                args.iter().find_map(|a| find_var_ty(a, name))
            }
            ExprNode::StringInterp { parts } => parts.iter().find_map(|p| match p {
                crate::expr::InterpPart::Expr { expr } => find_var_ty(expr, name),
                _ => None,
            }),
            ExprNode::Seq { exprs } => exprs.iter().find_map(|e| find_var_ty(e, name)),
            _ => None,
        }
    }

    #[test]
    fn body_typer_populates_param_refs_with_signature_types() {
        let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("types");
        let m = &methods[0];
        // `count` is used in the cond (`count == 1`) and in the else-branch
        // interpolation (`"#{count} ..."`); both should resolve to Int.
        assert_eq!(find_var_ty(&m.body, "count"), Some(Ty::Int));
        // `word` is used in both branches; should resolve to Str.
        assert_eq!(find_var_ty(&m.body, "word"), Some(Ty::Str));
    }

    #[test]
    fn body_typer_populates_literal_and_interp_types() {
        let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("types");
        let m = &methods[0];
        // The If as a whole unions its branches (both StringInterp → Str).
        assert_eq!(m.body.ty.as_ref(), Some(&Ty::Str));
    }
}
