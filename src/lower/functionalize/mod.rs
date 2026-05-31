//! Imperative → functional IR lowerings, gated to functional targets.
//!
//! Functional targets (Elixir today; Gleam/Erlang/Haskell later) can't
//! express Ruby's imperative control flow directly — no `while`, no
//! mutable variables, no `return`. Rather than teach each functional
//! emitter to de-imperative-ize at emit time, these passes rewrite the
//! IR into the functional vocabulary the IR *already has* (`Let`,
//! expression-`If`, `Send`, extra `MethodDef`s), leaving the emitter a
//! near-1:1 syntax map.
//!
//! The pass family (issue #29):
//!   1. `while`/`until`/`loop` → recursion  ← [`while_to_recursion`] (this slice)
//!   2. early `return` → expression          (currently in the elixir2 walker; migrate here)
//!   3. mutable local reassignment → SSA/`Let` (the cond-rebind fold; migrate here)
//!   4. `self.x =` instance mutation → struct-update return-threading
//!
//! **Gating.** This is *not* in the universal pre-emit pipeline — the
//! recursion form is strictly worse for imperative targets, which keep
//! the native `while`. Only functional emitters call [`functionalize`]
//! (the elixir2 overlay does, via its `elixir_units` transform).
//!
//! **Graceful degradation.** A pass only rewrites shapes it fully
//! supports; anything else is left untouched and falls through to the
//! emitter's `report_unsupported` catch-all (issue #28), which records
//! a structured diagnostic + emits a runtime stub. So the rest of the
//! program still transpiles, and coverage gaps self-report rather than
//! crashing.

pub mod local_accumulation;
pub mod mutation_to_struct_return;
pub mod while_to_recursion;

use std::collections::HashSet;

use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ty::Ty;

/// Apply the functional-lowering pass family to a set of library
/// classes. Called by functional emitters only. Each method may be
/// rewritten in place or expanded into several methods (e.g. a loop
/// method → an entry + a recursive helper).
pub fn functionalize(classes: Vec<LibraryClass>) -> Vec<LibraryClass> {
    classes
        .into_iter()
        .map(|mut class| {
            let methods = std::mem::take(&mut class.methods);
            // Fields backed by a Hash (`@data = {}` / `@data[k] = v`) so
            // the post-pass below can route reads of them to `Map.*`.
            let hash_fields = collect_hash_fields(&methods);
            class.methods = methods
                .into_iter()
                // while→recursion first (may split a method into entry +
                // helper), then thread instance mutation through the
                // results.
                .flat_map(while_to_recursion::transform_method)
                .map(mutation_to_struct_return::transform_method)
                .map(local_accumulation::transform_method)
                .map(|mut m| {
                    stamp_hash_fields(&mut m.body, &hash_fields);
                    m
                })
                .collect();
            class
        })
        .collect()
}

/// Names of `@ivar` fields whose storage is a Hash — detected from a
/// Hash-literal assignment (`@data = {}`) or an index write
/// (`@data[k] = v`). The elixir2 emitter routes Hash methods
/// (`key?`/`keys`/`delete`/…) to `Map.*` only on a Hash-typed receiver,
/// so stamping these fields' `__field__` reads as `Ty::Hash` is what
/// lets `record.data.key?(k)` become `Map.has_key?(record.data, k)`.
fn collect_hash_fields(methods: &[crate::dialect::MethodDef]) -> HashSet<String> {
    let mut out = HashSet::new();
    for m in methods {
        walk(&m.body, &mut |n| match &*n.node {
            ExprNode::Assign { target: LValue::Ivar { name }, value }
                if matches!(&*value.node, ExprNode::Hash { .. }) =>
            {
                out.insert(name.to_string());
            }
            ExprNode::Send { recv: Some(r), method, args, .. }
                if method.as_str() == "[]=" && args.len() == 2 =>
            {
                if let ExprNode::Ivar { name } = &*r.node {
                    out.insert(name.to_string());
                }
            }
            _ => {}
        });
    }
    out
}

/// Stamp `Ty::Hash` onto every `record.__field__(:f)` bridge whose
/// field `f` is a known Hash field, so the emitter's `recv_is_hash`
/// gate fires for reads of it. Runs after the rewrite passes (which is
/// when the bridges exist).
fn stamp_hash_fields(e: &mut Expr, hash_fields: &HashSet<String>) {
    let is_hash_field_read = match &*e.node {
        ExprNode::Send { method, args, .. }
            if method.as_str() == "__field__" && args.len() == 1 =>
        {
            matches!(
                &*args[0].node,
                ExprNode::Lit { value: Literal::Sym { value } }
                    if hash_fields.contains(value.as_str())
            )
        }
        _ => false,
    };
    if is_hash_field_read {
        e.ty = Some(Ty::Hash {
            key: Box::new(Ty::Untyped),
            value: Box::new(Ty::Untyped),
        });
    }
    walk_mut(e, &mut |c| stamp_hash_fields(c, hash_fields));
}

/// Visit each direct child of `e` (one level), applying `f`.
fn walk_mut(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    match &mut *e.node {
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            exprs.iter_mut().for_each(f)
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                f(r)
            }
            args.iter_mut().for_each(&mut *f);
            if let Some(b) = block {
                f(b)
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            f(cond);
            f(then_branch);
            f(else_branch);
        }
        ExprNode::While { cond, body, .. } => {
            f(cond);
            f(body);
        }
        ExprNode::BoolOp { left, right, .. } => {
            f(left);
            f(right);
        }
        ExprNode::Assign { value, .. } => f(value),
        ExprNode::Return { value } | ExprNode::Cast { value, .. } => f(value),
        ExprNode::Yield { args } => args.iter_mut().for_each(f),
        ExprNode::Lambda { body, .. } => f(body),
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries.iter_mut() {
                f(k);
                f(v);
            }
        }
        _ => {}
    }
}

/// Read-only single-level walk used by `collect_hash_fields`.
fn walk(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    match &*e.node {
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            exprs.iter().for_each(|x| walk(x, f))
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk(r, f)
            }
            args.iter().for_each(|a| walk(a, f));
            if let Some(b) = block {
                walk(b, f)
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, f);
            walk(then_branch, f);
            walk(else_branch, f);
        }
        ExprNode::While { cond, body, .. } => {
            walk(cond, f);
            walk(body, f);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, f);
            walk(right, f);
        }
        ExprNode::Assign { value, .. } => walk(value, f),
        ExprNode::Return { value } | ExprNode::Cast { value, .. } => walk(value, f),
        ExprNode::Yield { args } => args.iter().for_each(|a| walk(a, f)),
        ExprNode::Lambda { body, .. } => walk(body, f),
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk(k, f);
                walk(v, f);
            }
        }
        _ => {}
    }
}
