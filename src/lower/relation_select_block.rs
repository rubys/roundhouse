//! `<relation>.select { |r| … }` → `<relation>.filter { |r| … }`.
//!
//! `select` on a relation names two different methods. With column
//! specs it is the projection (`select(:id)` → `SELECT stories.id`),
//! answering the relation; with a block it is Enumerable's filter over
//! the loaded rows, answering an Array. The runtime carries both under
//! their Rails names — `select(*specs)` and `filter` — and this pass
//! sends the block form to the one that means it.
//!
//! WHY IT MATTERS BEYOND TIDINESS. Leaving both shapes on one name
//! forces `select` to answer `Relation | Array`, and a union return
//! makes every receiver downstream of it POLY. That is a correctness
//! hazard on the strict targets, not just a slow shape: spinel's
//! dynamic-dispatch path does not apply the braceless-keyword-args →
//! trailing-positional-Hash conversion, so
//! `Story.select(:id).where(merged_story_id: self.id)`
//! bound `where`'s optional `condition` parameter to its `nil` default
//! and the filter VANISHED — lobsters' `/s/:story_id` answered with the
//! comments of every merged story instead of its own (7 comments where
//! Rails renders 1). Nothing warned. Monomorphizing `select` puts that
//! call back on the static dispatch path, where the conversion happens.
//!
//! GATE. A receiver the analyzer typed `Ty::Relation` — and one it
//! could not type at all. The second half is not laziness: campfire
//! passes a relation into a private method and filters the PARAMETER —
//! `def extract_direct_memberships(all_memberships)` whose body is
//! `all_memberships.select { |m| m.room.direct? }` —
//! and that parameter is `Ty::Untyped` (a parameter with no call site
//! to constrain it stays an uninstantiated `Ty::Var`, which
//! `Ty::is_unknown` pairs with it). Missing it would leave a real
//! relation site on a `select` that no longer answers the block form —
//! which the runtime now raises on rather than answering wrongly, so a
//! missed site is loud, but loud is still broken. Rewriting it is safe
//! because `filter` is an EXACT alias of `select` in Ruby for every
//! receiver an untyped one could be (Array, Hash, Set, Struct,
//! Enumerator all define both, with the same return), so the rename
//! cannot change meaning where the guess is wrong.
//!
//! A receiver typed as something concrete and non-relational is left
//! alone. It cannot be the runtime Relation, its `select` already means
//! Enumerable's, and every target emitter has a path for that name
//! today — no reason to move it.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_relation_select_block_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "select" || block.is_none() || !args.is_empty() {
        return;
    }
    let rewritable = match recv.ty.as_ref() {
        // No annotation at all — the same "nothing is known here" the
        // predicate below names, spelled by absence.
        None => true,
        Some(t) => matches!(t, Ty::Relation { .. }) || t.is_unknown(),
    };
    if !rewritable {
        return;
    }
    *method = Symbol::from("filter");
}
