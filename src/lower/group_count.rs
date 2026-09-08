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
use crate::ty::Ty;

pub fn apply_group_count_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

/// The two parts of a grouped-count chain, for the callers that need
/// to look inside one.
pub struct GroupedCount<'a> {
    /// What the `group(...)` is called ON — the chain the group refines.
    /// `None` for an implicit-self body (`scope :by_x, -> {
    /// group(:x).count }`), which has no receiver to root a query at.
    pub base: Option<&'a Expr>,
    /// The `group(...)` arguments, as written.
    pub group_args: &'a [Expr],
}

/// Recognize a grouped count on `expr`, in EITHER spelling: the source
/// `rel.group(:col).count` and the `rel.group(:col).group_count` this
/// pass renames it to.
///
/// One predicate, two readers. The rename below asks it, and so does
/// `lower::arel::build` — which sees the chain at two different points
/// in the pipeline depending on the caller: the transpile driver runs
/// the post-analyze lowerings first (so the terminal arrives renamed),
/// while the emit-only harnesses call `Analyzer` + `emit` directly (so
/// it arrives as `count`). A recognizer keyed on one spelling folds the
/// query under one of those and emits a call to a `group` method no
/// target defines under the other. The QUERY is the same either way —
/// the rename exists to keep the runtime Relation's return
/// monomorphic, which is a fact about the runtime path, not about the
/// SQL.
pub fn grouped_count_parts(expr: &Expr) -> Option<GroupedCount<'_>> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return None;
    };
    if !matches!(method.as_str(), "count" | "group_count") || !args.is_empty() {
        return None;
    }
    let ExprNode::Send { recv: base, method: gm, args: group_args, block: None, .. } = &*recv.node
    else {
        return None;
    };
    if gm.as_str() != "group" {
        return None;
    }
    Some(GroupedCount { base: base.as_ref(), group_args })
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    // The SOURCE spelling only — `grouped_count_parts` accepts both, so
    // a second pass over an already-renamed tree is a no-op rather than
    // a rename of a rename.
    let is_source_count = matches!(
        &*expr.node,
        ExprNode::Send { method, .. } if method.as_str() == "count"
    );
    if is_source_count && grouped_count_parts(expr).is_some() {
        let ExprNode::Send { method, .. } = &mut *expr.node else { unreachable!() };
        *method = Symbol::from("group_count");
        // The rename is the only thing that knows this call is no longer
        // the `count` the source wrote, so it owns the renamed call's
        // type. Clearing it (what this line used to do) left a Send with
        // a known receiver and no type, which `analyze::diagnostics`
        // reads — it does not re-dispatch — as `send_dispatch_failed`:
        // "no known method `group_count` on Relation[Comment]", against
        // a name the app never wrote, a runtime method that exists
        // (`ActiveRecord::Relation#group_count`) and an RBS signature
        // that declares it. Every target failed the type gate on a
        // feature the pipeline ships (issue #77).
        //
        // A Hash already on the expression is the body-typer's answer
        // (`analyze::body::send::grouped_count_ty`, whose shape test
        // mirrors the match above), and it is the more precise one — it
        // carries the grouped COLUMN's key type. Keep it; fall back to
        // `relation.rbs`'s declared `Hash[untyped, Integer]` when the
        // typer had nothing to say (an untyped receiver, a shape its
        // schema read declined).
        if !matches!(expr.ty, Some(Ty::Hash { .. })) {
            expr.ty = Some(Ty::Hash {
                key: Box::new(Ty::Untyped),
                value: Box::new(Ty::Int),
            });
        }
    }
}
