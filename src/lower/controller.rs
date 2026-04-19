//! Controller-body lowering — shared Phase 4c helpers.
//!
//! When Rust + Crystal + Go each grew their own controller emitter
//! (Phase 4c), the same handful of shape-matching helpers appeared in
//! all three: split a controller body into public vs. private
//! actions, walk ivars, resolve `<assoc>` method names to target
//! model classes, recognise query-builder method chains, detect
//! `params` and `format`-bound receivers.
//!
//! These helpers live here rather than in each emitter because they
//! operate on dialect IR only — no target syntax. Each emitter then
//! does its own rendering on top (e.g., `let mut article = ...` vs.
//! `@article : Article = ...` vs. `article *Article`), consuming the
//! shared analysis.
//!
//! What's *not* here: the per-target `emit_controller_send_*`
//! dispatcher itself. That's target-specific rendering and stays put
//! for now; if/when a fourth Phase-4c target ships, a shared
//! classifier is the natural next lift (see `project_phase4c_lift_
//! candidates` memory for the sketch).

use std::collections::BTreeSet;

use crate::dialect::{Action, Controller, ControllerBodyItem};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;
use crate::naming;

/// Walk a controller's source-ordered body, partitioning actions into
/// those before the `private` marker vs. those after. Filters and
/// Unknown class-body calls are informational-only for emit and get
/// dropped; PrivateMarker is consumed as the partition point.
pub fn split_public_private(c: &Controller) -> (Vec<Action>, Vec<Action>) {
    let mut pubs = Vec::new();
    let mut privs = Vec::new();
    let mut seen_private = false;
    for item in &c.body {
        match item {
            ControllerBodyItem::PrivateMarker { .. } => seen_private = true,
            ControllerBodyItem::Action { action, .. } => {
                if seen_private {
                    privs.push(action.clone());
                } else {
                    pubs.push(action.clone());
                }
            }
            _ => {}
        }
    }
    (pubs, privs)
}

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

/// Query-builder method names that don't have a Phase 4c runtime.
/// Chains containing any of these collapse to an empty collection of
/// the chain's target model type at emit time. The set is the same on
/// every Phase-4c target — shape-shaped, not target-shaped.
///
/// `all` lives here too: the generated model has no `all` method, and
/// without this collapse each controller calling `Model.all` would
/// fail to compile on the typed targets.
pub fn is_query_builder_method(method: &str) -> bool {
    matches!(
        method,
        "all"
            | "includes"
            | "order"
            | "where"
            | "group"
            | "limit"
            | "offset"
            | "joins"
            | "distinct"
            | "select"
            | "pluck"
            | "first"
            | "last"
    )
}

/// Resolve a HasMany association name to its target model class.
/// `"comments"` → `"Comment"` iff `Comment` is in `known_models`.
///
/// Used by the `.build(hash)` / `.create(hash)` / `<assoc>.find(x)`
/// rewrites in every Phase-4c emitter — they all need to default-
/// construct the target, and the target's name falls out of
/// singularising the method name on the association chain.
pub fn singularize_to_model(assoc: &str, known_models: &[Symbol]) -> Option<Symbol> {
    let class = naming::singularize_camelize(assoc);
    known_models
        .iter()
        .find(|m| m.as_str() == class)
        .cloned()
}

/// Walk a chain of `Send`s left until hitting a `Const { path }`.
/// Returns the final path segment (the presumed class name) when it's
/// a known model. Used to pick the element type for
/// `Vec::<T>::new()` / `[] of T` / `[]*T{}` chain punts.
pub fn chain_target_class(e: &Expr, known_models: &[Symbol]) -> Option<Symbol> {
    let mut cur = e;
    loop {
        match &*cur.node {
            ExprNode::Const { path } => {
                let class = path.last()?;
                return known_models
                    .iter()
                    .find(|m| m.as_str() == class.as_str())
                    .cloned();
            }
            ExprNode::Send { recv: Some(r), .. } => cur = r,
            _ => return None,
        }
    }
}

/// True when an expression references the implicit `params` object —
/// a bare `Send { recv: None, method: "params", args: [] }`. Used by
/// the `params.expect(...)` / `params[k]` rewrites in every
/// Phase-4c emitter.
pub fn is_params_expr(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "params" && args.is_empty()
    )
}

/// True when an expression is the block parameter bound by
/// `respond_to do |format|` — today just the local `format` var. Used
/// to disambiguate `format.html { ... }` inside a respond_to block
/// from any unrelated `x.html` call outside.
pub fn is_format_binding(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Var { name, .. } if name.as_str() == "format"
    )
}
