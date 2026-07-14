//! Lambda-predicate `case` grounding: a `case` whose every `when` is
//! a single-param lambda literal rewrites to an if/elsif chain with
//! the lambda bodies inlined (lobsters' Search switches on
//! `base.klass` with `when ->(b) { b == Story }` — the spelling Rails
//! code uses for class-identity dispatch, since a bare `when Story`
//! on a class-valued scrutinee tests instance-of). Ruby dispatches
//! `pattern === scrutinee` and `Proc#===` calls the proc, so
//!
//!   case S
//!   when ->(b) { E(b) } then A
//!   else Z
//!   end
//!
//! is exactly `if E(S) then A else Z end`. Strict targets type a
//! first-class lambda param poorly through the `===` dispatch
//! (spinel#2439 pins it to INT), while the inlined comparison types
//! fine.
//!
//! Conservative: fires only when the scrutinee is a pure read chain
//! (Var/Ivar/Const/self/literal roots, zero-arg no-block sends) —
//! substitution re-evaluates it per arm — and every arm is either a
//! guard-free one-param lambda literal or the guard-free wildcard
//! `else`. Substitution skips nested lambdas that rebind the param.
//!
//! Purely shape-directed; runs on the post-analyze hook
//! (`apply_post_analyze_lowerings`) with its siblings so every target
//! consumes the grounded form.

use crate::app::App;
use crate::expr::{Arm, Expr, ExprNode, Literal, Pattern};
use crate::ident::Symbol;
use crate::span::Span;

pub fn apply_case_lambda_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    if !claims(expr) {
        return;
    }
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Case { scrutinee, arms } = node else { unreachable!() };
    let mut tail = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
    for arm in arms.into_iter().rev() {
        match arm.pattern {
            Pattern::Wildcard => tail = arm.body,
            Pattern::Expr { expr: pat } => {
                let ExprNode::Lambda { params, body, .. } = *pat.node else { unreachable!() };
                let mut cond = body;
                subst(&mut cond, &params[0], &scrutinee);
                tail = Expr::new(
                    arm.body.span,
                    ExprNode::If { cond, then_branch: arm.body, else_branch: tail },
                );
            }
            _ => unreachable!("claims() admitted lambda/wildcard arms only"),
        }
    }
    *expr.node = *tail.node;
    expr.ty = None;
}

fn claims(expr: &Expr) -> bool {
    let ExprNode::Case { scrutinee, arms } = &*expr.node else { return false };
    if !is_pure_read(scrutinee) || arms.is_empty() {
        return false;
    }
    arms.iter().enumerate().all(|(i, arm)| {
        if arm.guard.is_some() {
            return false;
        }
        match &arm.pattern {
            Pattern::Wildcard => i == arms.len() - 1,
            Pattern::Expr { expr } => matches!(
                &*expr.node,
                ExprNode::Lambda { params, block_param: None, .. } if params.len() == 1
            ),
            _ => false,
        }
    })
}

pub(crate) fn is_pure_read(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef
        | ExprNode::Lit { .. } => true,
        ExprNode::Send { recv: Some(r), args, block: None, .. } => {
            args.is_empty() && is_pure_read(r)
        }
        _ => false,
    }
}

fn subst(e: &mut Expr, param: &Symbol, scrutinee: &Expr) {
    // A nested lambda that rebinds the name shadows it.
    if matches!(
        &*e.node,
        ExprNode::Lambda { params, block_param, .. }
            if params.contains(param) || block_param.as_ref() == Some(param)
    ) {
        return;
    }
    if matches!(&*e.node, ExprNode::Var { name, .. } if name == param) {
        *e = scrutinee.clone();
        return;
    }
    e.node.for_each_child_mut(&mut |c| subst(c, param, scrutinee));
}
