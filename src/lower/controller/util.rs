//! Small leaf predicates and lookups shared across the controller
//! submodules — `is_params_expr`, `is_format_binding`, class/model
//! resolution, status-code mapping. No `Controller` reads; pure IR
//! structural matching.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::naming;

/// Resolve a HasMany association name to its target model class.
/// `"comments"` → `"Comment"` iff `Comment` is in `known_models`.
///
/// Used by the `.build(hash)` / `.create(hash)` / `<assoc>.find(x)`
/// rewrites in every Phase-4c emitter — they all need to default-
/// construct the target, and the target's name falls out of
/// singularising the method name on the association chain.
pub fn singularize_to_model(assoc: &str, known_models: &[Symbol]) -> Option<Symbol> {
    let class = naming::singularize_camelize(assoc);
    known_models
        .iter()
        .find(|m| m.as_str() == class)
        .cloned()
}

/// Walk a chain of `Send`s left until hitting a `Const { path }`.
/// Returns the final path segment (the presumed class name) when it's
/// a known model. Used to pick the element type for
/// `Vec::<T>::new()` / `[] of T` / `[]*T{}` chain punts.
pub fn chain_target_class(e: &Expr, known_models: &[Symbol]) -> Option<Symbol> {
    let mut cur = e;
    loop {
        match &*cur.node {
            ExprNode::Const { path } => {
                let class = path.last()?;
                return known_models
                    .iter()
                    .find(|m| m.as_str() == class.as_str())
                    .cloned();
            }
            ExprNode::Send { recv: Some(r), .. } => cur = r,
            _ => return None,
        }
    }
}

/// True when an expression references the implicit `params` object —
/// a bare `Send { recv: None, method: "params", args: [] }`. Used by
/// the `params.expect(...)` / `params[k]` rewrites in every
/// Phase-4c emitter.
pub fn is_params_expr(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "params" && args.is_empty()
    )
}

/// True when an expression is the block parameter bound by
/// `respond_to do |format|` — today just the local `format` var. Used
/// to disambiguate `format.html { ... }` inside a respond_to block
/// from any unrelated `x.html` call outside.
pub fn is_format_binding(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Var { name, .. } if name.as_str() == "format"
    )
}

/// Peel one `ExprNode::Lambda` layer — Ruby `do ... end` / `{ ... }`
/// ingests as a `Lambda` in the IR, but for emit purposes each block
/// is rendered as its body's statements, not as a lambda.
pub(super) fn unwrap_lambda(e: &Expr) -> &Expr {
    match &*e.node {
        ExprNode::Lambda { body, .. } => body,
        _ => e,
    }
}

/// Map a Rails status symbol (`:see_other`, `:unprocessable_entity`)
/// to its HTTP numeric code. Covers the codes the scaffold blog
/// templates use; unknown symbols fall back to 500.
pub fn status_sym_to_code(sym: &str) -> u16 {
    match sym {
        "ok" => 200,
        "created" => 201,
        "no_content" => 204,
        "see_other" => 303,
        "not_modified" => 304,
        "bad_request" => 400,
        "unauthorized" => 401,
        "not_found" => 404,
        "unprocessable_entity" => 422,
        _ => 500,
    }
}

/// Walk `render` / `redirect_to` keyword args for a `status:` key
/// and return its numeric string. Accepts symbol values
/// (`status: :see_other`) and integer literals (`status: 303`).
/// Returns `None` when no status is supplied, so callers can pick
/// their own default.
pub fn extract_status_from_kwargs(args: &[Expr]) -> Option<u16> {
    for arg in args {
        let ExprNode::Hash { entries, .. } = &*arg.node else { continue };
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                continue;
            };
            if key.as_str() != "status" {
                continue;
            }
            match &*v.node {
                ExprNode::Lit { value: Literal::Sym { value: s } } => {
                    return Some(status_sym_to_code(s.as_str()));
                }
                ExprNode::Lit { value: Literal::Int { value: n } } => {
                    return Some(*n as u16);
                }
                _ => return None,
            }
        }
    }
    None
}
