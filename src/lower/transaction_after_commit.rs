//! `….current_transaction.after_commit { body }` →
//! `….current_transaction.after_commit(-> { body; nil })`.
//!
//! Rails 7.2's `current_transaction.after_commit` runs its block once the
//! open transaction commits (at once when none is open). The runtime
//! holds the work until the COMMIT (`ActiveRecord::Base._after_commit`,
//! runtime/ruby/active_record/connection.rb), so it needs the block as a
//! VALUE: `ActiveRecord::Transaction#after_commit` takes it as a lambda,
//! `^() -> nil`, as `ActiveJob.enqueue` does (lower::job_class_side), and
//! RBS has no syntax for naming a block. The trailing nil gives every
//! held entry one type whatever the app's block answers.
//!
//! Only `after_commit` on a `current_transaction` receiver is rewritten.
//! The runtime models nothing else on the transaction handle, so any
//! other call on it stays the analyzer's error.

use crate::app::App;
use crate::expr::{BlockStyle, Expr, ExprNode, Literal};
use crate::ty::Ty;

pub fn apply_transaction_after_commit(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    rewrite_node(expr);
}

fn rewrite_node(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(recv), method, args, block, parenthesized } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "after_commit" || !args.is_empty() {
        return;
    }
    let ExprNode::Send { method: recv_method, .. } = &*recv.node else {
        return;
    };
    if recv_method.as_str() != "current_transaction" {
        return;
    }
    // A literal block taking no arguments; `after_commit(&work)` or a
    // block with parameters is left as the app wrote it.
    let Some(lambda) = block.as_ref() else {
        return;
    };
    let ExprNode::Lambda { params, rest_param, block_param, extra_params, .. } = &*lambda.node else {
        return;
    };
    if !params.is_empty() || rest_param.is_some() || block_param.is_some() || !extra_params.is_empty() {
        return;
    }
    let lambda = block.take().expect("checked above");
    let ExprNode::Lambda { body, .. } = *lambda.node else {
        unreachable!("checked above");
    };
    let span = lambda.span;
    let mut nil = Expr::new(span, ExprNode::Lit { value: Literal::Nil });
    nil.ty = Some(Ty::Nil);
    let mut work_body = Expr::new(span, ExprNode::Seq { exprs: vec![body, nil] });
    work_body.ty = Some(Ty::Nil);
    let work = Expr::new(
        span,
        ExprNode::Lambda {
            extra_params: Vec::new(),
            params: Vec::new(),
            rest_param: None,
            block_param: None,
            body: work_body,
            block_style: BlockStyle::Brace,
        },
    );
    args.push(work);
    *parenthesized = true;
}
