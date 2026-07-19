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

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;

/// Walk every hook body and ledger each dynamic relation chain.
/// Pure read — no rewrite; returns the diagnostics for the shared
/// residue channel.
pub fn apply_relation_residue_ledger(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |body| walk(body, false, &mut diags));
    diags
}

fn walk(e: &Expr, in_chain: bool, diags: &mut Vec<Diagnostic>) {
    if let ExprNode::Send { recv, method, args, block, .. } = &*e.node {
        let is_relation_head =
            matches!(e.ty.as_ref(), Some(Ty::Relation { .. })) && !in_chain;
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
            walk(r, spine, diags);
        }
        for a in args {
            walk(a, false, diags);
        }
        if let Some(b) = block {
            walk(b, false, diags);
        }
        return;
    }
    e.node.for_each_child(&mut |c| walk(c, false, diags));
}
