//! Controller-body ivar walking — collects which ivars appear as
//! reads vs. writes, used by pass-2 emitters to decide which
//! locals need defaults primed at the head of a generated handler.

use std::collections::BTreeSet;

use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;

/// Walk an action body collecting every ivar it touches. Returns two
/// sets (both deterministic):
///
/// - `assigned`: ivar names that appear on the LHS of an assignment
///   at some point in the body.
/// - `referenced`: ivar names in first-use order — every read *or*
///   write registers here. Used by the Rust emitter to compute
///   "referenced but never assigned" (the Rails `before_action`
///   filter would set these in the real runtime; Phase 4c primes them
///   with defaults).
///
/// Callers that only need the referenced list (Crystal, Go) pull
/// that half and ignore `assigned`.
pub fn walk_controller_ivars(body: &Expr) -> WalkedIvars {
    let mut out = WalkedIvars::default();
    walk(body, &mut out);
    out
}

#[derive(Default, Debug, Clone)]
pub struct WalkedIvars {
    /// ivar names that appear as the LHS of an assignment.
    pub assigned: BTreeSet<Symbol>,
    /// ivar names in first-use order across the body (read or write).
    pub referenced: Vec<Symbol>,
    /// Fast-lookup mirror of `referenced` to keep insertions O(log n)
    /// without losing ordering.
    seen: BTreeSet<Symbol>,
}

impl WalkedIvars {
    pub fn ivars_read_without_assign(&self) -> Vec<Symbol> {
        self.referenced
            .iter()
            .filter(|n| !self.assigned.contains(*n))
            .cloned()
            .collect()
    }
}

fn walk(e: &Expr, out: &mut WalkedIvars) {
    match &*e.node {
        ExprNode::Ivar { name } => {
            if out.seen.insert(name.clone()) {
                out.referenced.push(name.clone());
            }
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            out.assigned.insert(name.clone());
            if out.seen.insert(name.clone()) {
                out.referenced.push(name.clone());
            }
            walk(value, out);
        }
        ExprNode::Assign { value, .. } => walk(value, out),
        ExprNode::Seq { exprs } => {
            for child in exprs {
                walk(child, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, out);
            walk(then_branch, out);
            walk(else_branch, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, out);
            walk(right, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk(r, out);
            }
            for a in args {
                walk(a, out);
            }
            if let Some(b) = block {
                walk(b, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk(k, out);
                walk(v, out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk(el, out);
            }
        }
        ExprNode::Lambda { body, .. } => walk(body, out),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    walk(expr, out);
                }
            }
        }
        _ => {}
    }
}
