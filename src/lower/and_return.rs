//! `redirect_to(...) and return` grounding: `X and return v` rewrites
//! to `if X then return v end`, and a statement-position
//! `X or return v` to `if !X then return v end` (lobsters' signup
//! redirect and story-url guard). The Rails idiom puts a ReturnNode
//! in a BoolOp's value slot, which AOT targets reject; the If form
//! evaluates the operand once as the condition.
//!
//! The `and` form grounds in ANY position — when no return fires the
//! BoolOp's value is the falsy operand and the If's is nil, so the
//! only observable divergence is a caller reading a literal `false`
//! out of the idiom, a shape no corpus has (the spelling is control
//! flow by intent; controller-action tails discard their value by
//! contract). The `or` form yields the operand when TRUTHY — a real
//! value — so it grounds only where that value is provably unused:
//! non-final `Seq` elements, recursing through the branches of an
//! `If`/`Seq` that is itself a statement (the trailing-`if` spelling
//! wraps the BoolOp in an If).
//!
//! Purely shape-directed; runs on the post-analyze hook
//! (`apply_post_analyze_lowerings`) with its siblings so every target
//! consumes the grounded form.

use crate::app::App;
use crate::expr::{BoolOpKind, Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;

pub fn apply_and_return_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    if matches!(
        &*expr.node,
        ExprNode::BoolOp { op: BoolOpKind::And, right, .. }
            if matches!(&*right.node, ExprNode::Return { .. })
    ) {
        ground(expr);
    }
    if let ExprNode::Seq { exprs } = &mut *expr.node {
        let n = exprs.len();
        for e in exprs.iter_mut().take(n.saturating_sub(1)) {
            ground_stmt(e);
        }
    }
}

/// Statement position: the expression's own value is unused, so the
/// `or` form may ground too.
fn ground_stmt(e: &mut Expr) {
    match &mut *e.node {
        // A statement If's branch values are unused too.
        ExprNode::If { then_branch, else_branch, .. } => {
            ground_stmt(then_branch);
            ground_stmt(else_branch);
        }
        // A statement Seq's every element is a statement.
        ExprNode::Seq { exprs } => {
            for e in exprs {
                ground_stmt(e);
            }
        }
        ExprNode::BoolOp { right, .. } if matches!(&*right.node, ExprNode::Return { .. }) => {
            ground(e);
        }
        _ => {}
    }
}

fn ground(e: &mut Expr) {
    let span = e.span;
    let node = std::mem::replace(&mut *e.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::BoolOp { op, left, right, .. } = node else { unreachable!() };
    let cond = match op {
        BoolOpKind::And => left,
        BoolOpKind::Or => Expr::new(
            span,
            ExprNode::Send {
                recv: Some(left),
                method: Symbol::from("!"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        ),
    };
    *e.node = ExprNode::If {
        cond,
        then_branch: right,
        else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
    };
    e.ty = None;
}
