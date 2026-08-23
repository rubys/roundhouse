//! ActiveSupport's `Enumerable` extensions on a plain collection,
//! grounded to a runtime function instead of a core_ext reopen.
//!
//! Rails ships `index_by` by reopening `Enumerable`, which is a shape
//! only the CRuby overlay can host: the transpiled runtimes cannot
//! reopen a builtin, and spinel AOT cannot dispatch a user-defined
//! method on one. The runtime already answers `index_by` on
//! `ActiveRecord::Relation` (relation.rb), and the twin for everything
//! else is one module function taking the collection as an argument —
//! the same rule `active_support_ext.rb` states for `blank?`: the
//! receiver is evaluated exactly once, so a receiver with effects
//! grounds too, and no `respond_to?` is needed.
//!
//! campfire builds `Sound::INDEX = BUILTIN.index_by(&:name)` in a CLASS
//! BODY, so an ungrounded call is not a late NoMethodError on some
//! route — it fires while `app/models.rb` is being required and the
//! tree does not boot.
//!
//! WHAT IT DOES NOT REWRITE: a receiver the analyzer typed as a
//! Relation. That one has a real method with a real RBS signature, and
//! routing it through the module function would trade a typed call for
//! an untyped one to fix nothing.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_enumerable_ext_grounding(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let span = expr.span;
    let ExprNode::Send { recv, method, args, block, parenthesized } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "index_by" || !args.is_empty() || block.is_none() {
        return;
    }
    let Some(receiver) = recv.as_ref() else { return };
    if is_relation(receiver.ty.as_ref()) {
        return;
    }
    let receiver = recv.take().expect("checked above");
    *recv = Some(Expr::new(
        span,
        ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
    ));
    args.push(receiver);
    *parenthesized = true;
}

/// `Ty::Relation` under any element type, and through a nullable union
/// — the shape a scope chain leaves behind.
fn is_relation(ty: Option<&Ty>) -> bool {
    match ty {
        Some(Ty::Relation { .. }) => true,
        Some(Ty::Union { variants }) => variants.iter().any(|v| is_relation(Some(v))),
        _ => false,
    }
}
