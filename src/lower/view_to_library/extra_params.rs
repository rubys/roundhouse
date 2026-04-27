//! Collect bareword free-variable references from a view body so they
//! can surface as additional positional params on the emitted method.

use crate::expr::{Expr, ExprNode, InterpPart};

/// Walk the (already ivar-rewritten) body and collect bareword
/// references — `Send { recv: None, args: [], block: None }` and
/// `Var` reads — whose names are NOT the inferred view arg, NOT
/// `_buf`, and NOT a recognized view helper. Today this catches
/// `notice` / `alert` (Rails flash helpers parsed as bare Sends). They
/// surface as positional params on the emitted method so the body
/// type-checks under spinel-blog's runtime.
pub(super) fn collect_extra_params(body: &Expr, arg_name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut bound: Vec<String> = Vec::new();
    if !arg_name.is_empty() {
        bound.push(arg_name.to_string());
    }
    bound.push("_buf".to_string());
    bound.push("io".to_string());
    walk_for_extra(body, &bound, &mut out);
    out
}

fn walk_for_extra(e: &Expr, bound: &[String], out: &mut Vec<String>) {
    match &*e.node {
        ExprNode::Var { name, .. } => {
            let n = name.as_str();
            if !bound.iter().any(|b| b == n) && !out.iter().any(|x| x == n) && is_flash_name(n) {
                out.push(n.to_string());
            }
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            if recv.is_none() && args.is_empty() && block.is_none() {
                let n = method.as_str();
                if !bound.iter().any(|b| b == n)
                    && !out.iter().any(|x| x == n)
                    && is_flash_name(n)
                {
                    out.push(n.to_string());
                }
            }
            if let Some(r) = recv {
                walk_for_extra(r, bound, out);
            }
            for a in args {
                walk_for_extra(a, bound, out);
            }
            if let Some(b) = block {
                walk_for_extra(b, bound, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                walk_for_extra(e, bound, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk_for_extra(cond, bound, out);
            walk_for_extra(then_branch, bound, out);
            walk_for_extra(else_branch, bound, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk_for_extra(left, bound, out);
            walk_for_extra(right, bound, out);
        }
        ExprNode::Assign { value, .. } => walk_for_extra(value, bound, out),
        ExprNode::Lambda { body, params, .. } => {
            let mut inner_bound = bound.to_vec();
            for p in params {
                inner_bound.push(p.as_str().to_string());
            }
            walk_for_extra(body, &inner_bound, out);
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk_for_extra(el, bound, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk_for_extra(k, bound, out);
                walk_for_extra(v, bound, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    walk_for_extra(expr, bound, out);
                }
            }
        }
        _ => {}
    }
}

/// Today's heuristic for "this bareword is a Rails flash helper that
/// should surface as a method parameter." Conservative — any other
/// unknown bareword stays as a free reference and the analyzer / type
/// checker is responsible for diagnosing it. Expand the set as
/// fixtures introduce more flash-style helpers.
fn is_flash_name(n: &str) -> bool {
    matches!(n, "notice" | "alert")
}
