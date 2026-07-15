//! Grouped-count grounding: `rel.group(:col).count` renames its
//! terminal to `group_count` — Rails' grouped count returns a Hash of
//! group-key => COUNT, a different type than the scalar `count`.
//! Splitting the name keeps both returns monomorphic (a count that
//! answers Integer-or-Hash is exactly the polymorphic-API shape the
//! runtime avoids); the runtime's `group_count` builds the
//! SELECT … GROUP BY and hydrates the Hash.
//!
//! Shape-directed: a zero-arg block-less `count` whose receiver is a
//! `group(...)` send (the corpus spelling — `group` directly before
//! the terminal). Runs on the post-analyze hook with its siblings so
//! every target consumes the grounded form.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_group_count_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let matches = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "count"
                && args.is_empty()
                && matches!(&*r.node,
                    ExprNode::Send { method: gm, .. } if gm.as_str() == "group")
    );
    if matches {
        let ExprNode::Send { method, .. } = &mut *expr.node else { unreachable!() };
        *method = Symbol::from("group_count");
        expr.ty = None;
    }
}
