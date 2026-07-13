//! `request[<key>]` → `params[<key>]` in controller bodies.
//!
//! Rails' `ActionDispatch::Request#[]` is params delegation and nothing
//! else — rack's `def [](key); params[key.to_s]; end`, and our CRuby
//! overlay Request implements the same two-liner. Controllers on the
//! strict-target trees have no Request object yet (the two-layer
//! Request split is still pending), so the indexed read types unknown
//! and spinel AOT refuses the downstream equality (lobsters'
//! `request[:format] == "rss"` RSS-token filter). Grounding the read to
//! `params[<key>]` routes it through the established params machinery —
//! typed on every target, key-lowered on the ruby path — and drops the
//! Request dependency for the one Request member that is pure params
//! delegation. The rewrite swaps only the receiver (`request` →
//! `params`); the index shape and key expression stay as ingested, so
//! the result is indistinguishable from a source `params[<key>]` read.
//!
//! Controller bodies ONLY: helper modules and views also mention bare
//! `request`, but `params` isn't in scope there (helpers get the
//! `Current.request` rewrite on the CRuby path instead). Non-literal
//! keys rewrite too — the delegation is key-independent.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_request_index_lowering(app: &mut App) {
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => {
                    for (_name, default) in &mut action.opt_params {
                        rewrite_request_index(default);
                    }
                    rewrite_request_index(&mut action.body);
                }
                crate::dialect::ControllerBodyItem::Unknown { expr, .. } => {
                    rewrite_request_index(expr)
                }
                _ => {}
            }
        }
    }
}

/// A bare zero-arg `request` send (implicit self receiver).
fn is_bare_request(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "request" && args.is_empty()
    )
}

fn to_bare_params(e: &mut Expr) {
    if let ExprNode::Send { method, .. } = &mut *e.node {
        *method = Symbol::from("params");
    }
    // The receiver's stamped type (if any) described the request
    // object; the params read gets re-typed downstream like any other
    // params site.
    e.ty = None;
}

fn rewrite_request_index(expr: &mut Expr) {
    expr.node
        .for_each_child_mut(&mut rewrite_request_index);
    // Indexed READS are `Send "[]"` (`LValue::Index` is the write side,
    // and `request[k] = v` isn't a thing).
    if let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &mut *expr.node {
        if method.as_str() == "[]" && args.len() == 1 && is_bare_request(recv) {
            to_bare_params(recv);
        }
    }
}
