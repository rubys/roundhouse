//! Shared lowering: counted Relation terminals.
//!
//! `first(n)` / `last(n)` on a receiver the analyzer typed as a Relation
//! become the runtime's `first_n` / `last_n`.
//!
//! The runtime splits the counted forms from the bare ones because one
//! method cannot carry both return types on a strict target: `last`
//! answers a record or nil, `last(n)` an Array (see
//! `scope_chain::counted_terminal`, which renames the chains it can
//! prove relations syntactically, on the Ruby emit path). A typed
//! receiver needs no syntax to prove it: campfire's `Page.load` takes
//! its relation as a parameter, and once `send_dispatch` grounds its
//! `public_send(direction, size)` the arms read
//! `relation.skip_preloading!.last(size)`, which reached the zero-arg
//! `last` and raised ArgumentError.
//!
//! `rel.count > n` becomes `rel.more_than?(n)` — `SELECT 1 LIMIT 1
//! OFFSET n` matching `count_sql`'s FROM/JOIN/WHERE, not a COUNT(*)
//! and not `offset(n).exists?` (`offset` mutates; loaded `exists?`
//! ignores it). Only a non-negative integer literal or a Const
//! (`PAGE_SIZE`) is rewritten, so an effectful right-hand side keeps
//! source evaluation order.
//!
//! Only `Ty::Relation` counts. `Ty::Array` receivers keep `first(n)`
//! and `Array#count`. A block form is Enumerable#detect and is left
//! alone.

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
    let parenthesized = match &*e.node {
        ExprNode::Send { parenthesized, .. } => *parenthesized,
        _ => return false,
    };
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
