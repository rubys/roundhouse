//! ActiveSupport's `Object#in?` → `<arg>.include?(<recv>)`.
//!
//! The mirror of `exclude_predicate` beside this file, and it reaches
//! the same wall for the same reason: `in?` is a core_ext reopen on
//! Object whose whole body is `collection.include?(self)`, and no
//! transpiled runtime can reopen the builtin. The analyzer has typed
//! the call `Ty::Bool` since before anything implemented it
//! (`analyze::body::send`), which is the shape that hides this kind of
//! gap — the type table answers, so nothing looks missing until a body
//! runs. campfire's `Opengraph::Metadata::Fetching` writes three of
//! them, and all ten tests in `opengraph_metadata_test` died on the
//! first.
//!
//! Unlike `exclude?` this rewrite SWAPS receiver and argument, so it
//! also swaps their evaluation ORDER: `f().in?(g())` evaluates `f`
//! first, `g().include?(f())` evaluates `g` first. Every site in the
//! corpus has a pure receiver and a constant collection
//! (`content_type.in?(ALLOWED_IMAGE_CONTENT_TYPES)`), and a rewrite
//! that preserved order would have to bind a temporary — a statement
//! where this pass only has an expression. Named here rather than
//! silently accepted; a site with two effectful sides is the one to
//! come back for.
//!
//! Same wholesale opt-out as `exclude?`: an app defining its own `in?`
//! means something else by the name, and per-class checking is no help
//! when the receiver is the value rather than the definer.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_in_predicate_lowering(app: &mut App) {
    if app_defines_in(app) {
        return;
    }
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

/// Does any app class define its own `in?`? Then the name doesn't mean
/// ActiveSupport's and the pass stands down.
fn app_defines_in(app: &App) -> bool {
    let is_in = |n: &Symbol| n.as_str() == "in?";
    app.models.iter().any(|m| {
        m.body.iter().any(|item| {
            matches!(item, crate::dialect::ModelBodyItem::Method { method, .. }
                if is_in(&method.name))
        })
    }) || app
        .library_classes
        .iter()
        .any(|c| c.methods.iter().any(|m| is_in(&m.name)))
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "in?" || args.len() != 1 {
        return;
    }
    let span = expr.span;
    let ExprNode::Send { recv, args, parenthesized, .. } = (*expr.node).clone() else {
        return;
    };
    let Some(recv) = recv else { return };
    let Some(collection) = args.into_iter().next() else { return };
    // Stamped Bool rather than left for re-inference, like the
    // `exclude?` twin: `include?` is a total predicate and the strict
    // emitters read the stamped type to pick a boolean context.
    let mut include = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(collection),
            method: Symbol::from("include?"),
            args: vec![recv],
            block: None,
            parenthesized,
        },
    );
    include.ty = Some(crate::ty::Ty::Bool);
    *expr = include;
}
