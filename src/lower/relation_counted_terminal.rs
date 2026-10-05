//! Shared lowering: counted Relation terminals.
//!
//! Typed `first(n)` / `last(n)` rename to `first_n` / `last_n` — one
//! method cannot carry both a record and an Array on a strict target.
//! Syntactic chains go through `scope_chain::counted_terminal`.
//!
//! `rel.count > n` → `rel.more_than?(n)` (`SELECT 1 LIMIT 1 OFFSET n`).
//! Not `offset(n).exists?`: offset mutates, and loaded exists? ignores
//! it. Only a non-negative integer literal or Const (`PAGE_SIZE`).
//! Arrays and block forms stay Enumerable.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_relation_counted_terminals(app: &mut App) {
    super::for_each_hook_body(app, &mut |body| rewrite(body));
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut |c| rewrite(c));
    rewrite_count_gt_when(e, |rel| matches!(rel.ty, Some(Ty::Relation { .. })));
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *e.node {
        if args.len() == 1 && matches!(r.ty, Some(Ty::Relation { .. })) {
            let renamed = match method.as_str() {
                "first" => "first_n",
                "last" => "last_n",
                _ => return,
            };
            *method = Symbol::from(renamed);
        }
    }
}

/// `rel.count > n` → `rel.more_than?(n)` when `rel_ok` proves the
/// count's receiver is a Relation. Used from this pass (typed) and
/// from `scope_chain` (syntactic `__rel` / chain).
pub(crate) fn rewrite_count_gt_when(e: &mut Expr, rel_ok: impl Fn(&Expr) -> bool) -> bool {
    if !is_count_gt_shape(e, &rel_ok) {
        return false;
    }
    let parenthesized = true;
    let ExprNode::Send { recv: Some(count_e), args, .. } = &mut *e.node else {
        return false;
    };
    let threshold = args.remove(0);
    let ExprNode::Send { recv, .. } = &mut *count_e.node else {
        return false;
    };
    let Some(rel) = recv.take() else {
        return false;
    };
    *e.node = ExprNode::Send {
        recv: Some(rel),
        method: Symbol::from("more_than?"),
        args: vec![threshold],
        block: None,
        parenthesized,
    };
    e.ty = Some(Ty::Bool);
    true
}

fn is_count_gt_shape(e: &Expr, rel_ok: &impl Fn(&Expr) -> bool) -> bool {
    let ExprNode::Send { recv: Some(count_e), method, args, block: None, .. } = &*e.node else {
        return false;
    };
    if method.as_str() != ">" || args.len() != 1 || !is_count_threshold(&args[0]) {
        return false;
    }
    let ExprNode::Send {
        recv: Some(rel),
        method: count_m,
        args: count_args,
        block: None,
        ..
    } = &*count_e.node
    else {
        return false;
    };
    count_m.as_str() == "count" && count_args.is_empty() && rel_ok(rel)
}

fn is_count_threshold(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Lit { value: Literal::Int { value } } => *value >= 0,
        ExprNode::Const { .. } => true,
        _ => false,
    }
}
