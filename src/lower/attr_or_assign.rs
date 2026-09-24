//! `recv.attr ||= value` on a receiver the analyzer could not type —
//! desugared to `recv.attr || (recv.attr = value)` (and `&&=` to the
//! `&&` twin).
//!
//! Ruby defines the compound form as exactly that when `recv` is
//! evaluated once, so the rewrite fires only on a pure-read receiver
//! (a local, an ivar, a zero-arg read chain), where evaluating it twice
//! is the same as once.
//!
//! Only for an UNTYPED receiver, because that is the one spinel refuses
//! ("unsupported call-or-write (non-object)"): its CallOrWrite lowering
//! needs a concrete class to read and write the slot, while the plain
//! read and the plain writer dispatch through a poly receiver like any
//! other send. A typed receiver keeps the native compound form every
//! emitter already renders. lobsters' `CommentVoteHydrator#[]` is the
//! case: `comment.current_vote ||= @votes[comment.id]` on a parameter
//! nothing types.

use crate::app::App;
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, LValue, OpAssignOp};

pub fn apply_attr_or_assign_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::OpAssign { target: LValue::Attr { recv, name }, op, value } = &*expr.node else {
        return;
    };
    let kind = match op {
        OpAssignOp::OrOr => BoolOpKind::Or,
        OpAssignOp::AndAnd => BoolOpKind::And,
        _ => return,
    };
    let untyped = recv.ty.as_ref().is_none_or(|t| t.is_unknown());
    if !untyped || !super::case_lambda::is_pure_read(recv) {
        return;
    }
    let span = expr.span;
    let mut read = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv.clone()),
            method: name.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    read.ty = expr.ty.clone();
    let mut write = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Attr { recv: recv.clone(), name: name.clone() },
            value: value.clone(),
        },
    );
    write.ty = value.ty.clone();
    let ty = expr.ty.clone();
    *expr = Expr::new(
        span,
        ExprNode::BoolOp { op: kind, surface: BoolOpSurface::Symbol, left: read, right: write },
    );
    expr.ty = ty;
}
