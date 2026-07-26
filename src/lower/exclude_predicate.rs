//! ActiveSupport's `Enumerable#exclude?` → `!<recv>.include?(arg)`.
//!
//! `exclude?` is a core_ext reopen whose entire body is `!include?(x)`
//! — the same shape as the `blank?` family beside this file, and it
//! reaches the same wall: no transpiled runtime can reopen the builtin,
//! and spinel AOT refuses the call outright once the receiver has a
//! static type (`unsupported call: exclude?`).
//!
//! Unlike `blank?` this needs no per-type grounding. `blank?` means
//! different things to a String, an Array and a Time, so that pass has
//! to switch on the receiver's type and warn where it can't. `exclude?`
//! is the exact negation of `include?` for every receiver AS defines it
//! on, so the rewrite is unconditional and total — which also means it
//! grounds untyped receivers the blank pass has to leave alone.
//!
//! The one thing that can make it wrong is an app defining its own
//! `exclude?`, so the pass disables itself wholesale if any app class
//! does. That is deliberately coarser than the blank pass's per-class
//! check: `exclude?` sites are overwhelmingly on collections rather
//! than on `self`, so a receiver type rarely names the defining class,
//! and a global opt-out is the honest reading of "this app means
//! something else by that name".
//!
//! Views ARE walked here, unlike the blank pass. Its exclusion exists
//! because the per-target view-cond predicate rewrites match the
//! original `blank?`/`present?` shapes and would miss a rewritten one;
//! no such machinery keys on `exclude?`, and lobsters' only view site
//! (`_threads`: `@read_by_notifications.exclude?(comment.id)`) is a
//! compile stop as soon as the local is typed.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_exclude_predicate_lowering(app: &mut App) {
    if app_defines_exclude(app) {
        return;
    }
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

/// Does any app class define its own `exclude?`? Then the name doesn't
/// mean ActiveSupport's and the pass stands down.
fn app_defines_exclude(app: &App) -> bool {
    let is_exclude = |n: &Symbol| n.as_str() == "exclude?";
    app.models.iter().any(|m| {
        m.body.iter().any(|item| {
            matches!(item, crate::dialect::ModelBodyItem::Method { method, .. }
                if is_exclude(&method.name))
        })
    }) || app
        .library_classes
        .iter()
        .any(|c| c.methods.iter().any(|m| is_exclude(&m.name)))
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "exclude?" || args.len() != 1 {
        return;
    }
    let span = expr.span;
    let ExprNode::Send { recv, args, parenthesized, .. } = (*expr.node).clone() else {
        return;
    };
    // Both nodes are stamped Bool rather than left for re-inference:
    // `include?` and `!` are total predicates, and the strict emitters
    // read the stamped type to pick a boolean context.
    let mut include = Expr::new(
        span,
        ExprNode::Send {
            recv,
            method: Symbol::from("include?"),
            args,
            block: None,
            parenthesized,
        },
    );
    include.ty = Some(crate::ty::Ty::Bool);
    let mut negated = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(include),
            method: Symbol::from("!"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    );
    negated.ty = Some(crate::ty::Ty::Bool);
    *expr = negated;
}
