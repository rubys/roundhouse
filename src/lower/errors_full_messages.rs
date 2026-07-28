//! `record.errors.full_messages` → `record.errors`.
//!
//! The shared framework runtime's `errors` is already the array of
//! full-message strings — `validates` bakes humanized text at lower
//! time ("Short can't be blank") and hand-written `errors.add` calls
//! ground into the same shape (see [`super::errors_add`], which owns
//! that half and documents the accumulator invariant). Rails' extra
//! `.full_messages` hop is therefore an identity on our runtime, and
//! leaving it in place is not harmless: `Array[String]` has no
//! `full_messages`, so every strict target gets a hard unresolved-method
//! error at a site whose meaning we already know.
//!
//! Scope is `for_each_hook_body` plus view bodies — the construct has
//! real presence in both (lobsters reads it from controllers, from
//! models, and from an `application_helper` error-list builder that
//! views call). That matches [`super::exclude_predicate`], the other
//! total rewrite with a view presence.
//!
//! An equivalent rewrite already existed as `rewrite_errors_full_messages`
//! in the Roda ingest, applied to view bodies only. That copy is left in
//! place deliberately: it runs at INGEST, this runs post-analyze, and
//! the fold is idempotent (after either pass no `full_messages` send
//! survives for the other to match), so the lanes can overlap without
//! interfering. Retiring it is a separate cleanup with its own gate.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_errors_full_messages_lowering(app: &mut App) {
    if app_defines_full_messages(app) {
        return;
    }
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

/// Does any app class define its own `full_messages`? Then the name
/// doesn't mean Rails' and the pass stands down, rather than folding
/// away a real call. Mirrors `exclude_predicate`'s guard.
fn app_defines_full_messages(app: &App) -> bool {
    let is_fm = |n: &Symbol| n.as_str() == "full_messages";
    app.models.iter().any(|m| {
        m.body.iter().any(|item| {
            matches!(item, crate::dialect::ModelBodyItem::Method { method, .. }
                if is_fm(&method.name))
        })
    }) || app
        .library_classes
        .iter()
        .any(|lc| lc.methods.iter().any(|m| is_fm(&m.name)))
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    // `<recv>.errors.full_messages` — zero-arg, block-free on both hops.
    // The receiver being an `errors` reader is what makes this our
    // accumulator rather than some other object's method of the same
    // name; the receiver of `errors` itself is unconstrained, so both
    // `errors.full_messages` (a model validating itself) and
    // `record.errors.full_messages` (a controller reading another
    // object's) fold, and the surviving expression keeps that receiver.
    let replacement = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "full_messages"
                && args.is_empty()
                && matches!(
                    &*r.node,
                    ExprNode::Send { method: em, args: ea, block: None, .. }
                        if em.as_str() == "errors" && ea.is_empty()
                ) =>
        {
            Some((*r.node).clone())
        }
        _ => None,
    };
    if let Some(node) = replacement {
        expr.node = Box::new(node);
    }
}
