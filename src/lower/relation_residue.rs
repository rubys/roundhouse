//! Tier-3 relation-residue ledger (relation-type-plan R6).
//!
//! A `Ty::Relation`-typed chain that survives to this point stays
//! *dynamic*: specialization has not folded it into direct SQL, so it
//! executes on the runtime `ActiveRecord::Relation` in the ruby-family
//! targets and reports `Unsupported` if it reaches a strict target's
//! emitter. That is working behavior, not an error — but it is exactly
//! the residue the erasure-first design wants ledgered: the count of
//! these sites (per app) is the input to the decision on whether
//! tier-2/tier-3 machinery (enumerable branch shapes, runtime relation
//! classes elsewhere) would ever pay for itself. One warning per chain
//! head, construct id `dynamic_relation` — greppable, and countable
//! via `roundhouse-check` like every other `LowerResidue`.
//!
//! Counting rule: the OUTERMOST Relation-typed Send of a receiver
//! chain is the chain head and gets the single entry; the spine below
//! it is the same chain and is suppressed. Argument and block
//! positions start fresh chains. Bodies covered are the post-analyze
//! hook's (`for_each_hook_body`) — views are lowered later and keep
//! their own channel.
//!
//! EXCEPT a head the Arel builder will fold. This pass runs inside
//! `apply_post_analyze_lowerings`, but `rewrite_arel_in_expr` runs
//! later still — inside `controller_to_library` / `model_to_library`.
//! So "survives to this point" is not the same as "stays dynamic": a
//! `Model.where(...)` chain is Relation-typed here and direct SQL by
//! emit. The pass therefore ASKS `try_build_arel` — the recognizer
//! itself, not a re-derivation of its rule — whether each head would
//! lift, and counts only the ones that won't. Before class-side chain
//! starts converged onto `Ty::Relation` this gap was invisible, since
//! the only Relation-typed heads were scope-rooted and `try_build_arel`
//! doesn't recognize a scope root anyway.
//!
//! A head with an IMPLICIT receiver (`scope :recent, -> { limit(10) }`)
//! is not folded and is correctly counted: it lowers to a call on the
//! runtime `ActiveRecord::Relation` (`__rel.limit(10)`).

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode};
use crate::ident::ClassId;
use crate::lower::model_associations::AssociationEdge;
use crate::schema::Schema;
use crate::ty::Ty;

/// What `walk` needs to answer "would the Arel builder lift this?" —
/// exactly `try_build_arel_with_assocs`'s inputs, carried together
/// because `for_each_hook_body` borrows the app mutably.
struct FoldCtx<'a> {
    schema: Schema,
    registry: &'a HashMap<ClassId, ClassInfo>,
    assocs: Vec<AssociationEdge>,
}

/// Walk every hook body and ledger each dynamic relation chain.
/// Pure read — no rewrite; returns the diagnostics for the shared
/// residue channel.
pub fn apply_relation_residue_ledger(
    app: &mut App,
    registry: &HashMap<ClassId, ClassInfo>,
) -> Vec<Diagnostic> {
    // Cloned/computed up front: `for_each_hook_body` takes `&mut App`,
    // so nothing can stay borrowed out of it during the walk.
    let ctx = FoldCtx {
        schema: app.schema.clone(),
        registry,
        assocs: crate::lower::model_associations::compute_association_graph(app),
    };
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |body| walk(body, false, &ctx, &mut diags));
    diags
}

/// Will the Arel builder lift this Send to direct SQL before emit?
fn folds_to_sql(e: &Expr, ctx: &FoldCtx) -> bool {
    crate::lower::arel::try_build_arel_with_assocs(e, &ctx.schema, ctx.registry, &ctx.assocs)
        .is_some()
}

fn walk(e: &Expr, in_chain: bool, ctx: &FoldCtx, diags: &mut Vec<Diagnostic>) {
    if let ExprNode::Send { recv, method, args, block, .. } = &*e.node {
        let is_relation_head = matches!(e.ty.as_ref(), Some(Ty::Relation { .. }))
            && !in_chain
            && !folds_to_sql(e, ctx);
        if is_relation_head {
            let of = match e.ty.as_ref() {
                Some(Ty::Relation { of }) => of.0.as_str().to_string(),
                _ => String::new(),
            };
            diags.push(crate::lower::residue_diagnostic(
                "relation_residue",
                "dynamic_relation",
                e.span,
                "unspecialized_relation_chain",
                format!(
                    "relation chain stays dynamic (`{}` returns Relation[{of}] — \
                     not folded to SQL at transpile time); executes on the \
                     runtime Relation in ruby-family targets, unsupported at \
                     strict-target emit",
                    method.as_str(),
                ),
            ));
        }
        // The receiver spine below a Relation-typed Send is the same
        // chain — suppress duplicate entries. Everything else starts
        // a fresh chain.
        let spine = in_chain
            || matches!(e.ty.as_ref(), Some(Ty::Relation { .. }));
        if let Some(r) = recv {
            walk(r, spine, ctx, diags);
        }
        for a in args {
            walk(a, false, ctx, diags);
        }
        if let Some(b) = block {
            walk(b, false, ctx, diags);
        }
        return;
    }
    e.node.for_each_child(&mut |c| walk(c, false, ctx, diags));
}
