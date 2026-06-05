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

use std::collections::HashMap;

use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ty::Ty;

/// Apply the functional-lowering pass family to a set of library
/// classes. Called by functional emitters only. Each method may be
/// rewritten in place or expanded into several methods (e.g. a loop
/// method → an entry + a recursive helper).
pub fn functionalize(classes: Vec<LibraryClass>) -> Vec<LibraryClass> {
    functionalize_with_external_duals(classes, &std::collections::HashSet::new())
}

/// Like [`functionalize`], but seeds each class's dual-return registry
/// with `external_duals` — dual `{record, value}` methods defined in
/// OTHER classes (e.g. a model's `save`/`update`/`destroy`) that a
/// controller action calls on a typed field (`@article.save`). The
/// per-class registry only sees the current class's methods, so a
/// cross-class dual call would otherwise miss its tuple destructure and
/// the `if @article.save` condition would test the whole `{record, bool}`
/// tuple (always truthy → invalid records still redirect).
pub fn functionalize_with_external_duals(
    classes: Vec<LibraryClass>,
    external_duals: &std::collections::HashSet<String>,
) -> Vec<LibraryClass> {
    classes
        .into_iter()
        .map(|mut class| {
            let methods = std::mem::take(&mut class.methods);
            // `@ivar` field → container Ty (`@data = {}` → Hash,
            // `@errors = []` → Array), so the post-pass below can route
            // reads of them to `Map.*` / `Enum.*`.
            let field_types = collect_field_types(&methods);
            // while→recursion first (may split a method into entry +
            // helper), then thread instance mutation through the results.
            let after_while: Vec<_> =
                methods.into_iter().flat_map(while_to_recursion::transform_method).collect();
            // Classify methods (record-returning vs dual-return) up front
            // so a self-call can be rebound at its call site — the body
            // of `valid?` must rebind its `validate` call, `save` must
            // destructure its `valid?` call, etc.
            let mut registry = mutation_to_struct_return::compute_registry(&after_while);
            // Cross-class dual methods (a controller calling a model's
            // `save`/`update`) — recorded separately from this class's own
            // duals so their CALL SITES destructure the tuple, WITHOUT
            // mis-classifying a same-named method defined here (the
            // controller's own `update` action is record-returning).
            registry.external_dual.extend(external_duals.iter().cloned());
            class.methods = after_while
                .into_iter()
                .map(|m| mutation_to_struct_return::transform_method(m, &registry))
                .map(local_accumulation::transform_method)
                .map(|mut m| {
                    stamp_field_types(&mut m.body, &field_types);
                    m
                })
                .collect();
            class
        })
        .collect()
}

/// `@ivar` field → its container `Ty`, inferred from a literal
/// initializer: `@data = {}` → `Ty::Hash`, `@errors = []` → `Ty::Array`;
/// an index write (`@data[k] = v`) also implies Hash. The elixir2
/// emitter routes container methods (`key?`/`keys`/`empty?`/…) to
/// `Map.*`/`Enum.*` only on a typed receiver, so stamping a field's
/// `__field__` reads with this Ty is what lets `record.data.key?(k)`
/// become `Map.has_key?(record.data, k)` and `record.errors.empty?`
/// become `Enum.empty?(record.errors)` — the analyzer doesn't type
/// these ivars in library mode.
fn collect_field_types(methods: &[crate::dialect::MethodDef]) -> HashMap<String, Ty> {
    let mut out = HashMap::new();
    let hash_ty = || Ty::Hash { key: Box::new(Ty::Untyped), value: Box::new(Ty::Untyped) };
    let array_ty = || Ty::Array { elem: Box::new(Ty::Untyped) };
    for m in methods {
        walk(&m.body, &mut |n| match &*n.node {
            ExprNode::Assign { target: LValue::Ivar { name }, value } => match &*value.node {
                ExprNode::Hash { .. } => {
                    out.insert(name.to_string(), hash_ty());
                }
                ExprNode::Array { .. } => {
                    out.insert(name.to_string(), array_ty());
                }
                _ => {}
            },
            ExprNode::Send { recv: Some(r), method, args, .. }
                if method.as_str() == "[]=" && args.len() == 2 =>
            {
                if let ExprNode::Ivar { name } = &*r.node {
                    out.entry(name.to_string()).or_insert_with(hash_ty);
                }
            }
            _ => {}
        });
    }
    out
}

/// Stamp the inferred container `Ty` onto every `record.__field__(:f)`
/// bridge whose field `f` is in `field_types`, so the emitter's
/// `recv_is_hash`/`recv_is_array` gates fire for reads of it. Runs after
/// the rewrite passes (which is when the bridges exist).
fn stamp_field_types(e: &mut Expr, field_types: &HashMap<String, Ty>) {
    let field_ty = match &*e.node {
        ExprNode::Send { method, args, .. }
            if method.as_str() == "__field__" && args.len() == 1 =>
        {
            match &*args[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => {
                    field_types.get(value.as_str()).cloned()
                }
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(ty) = field_ty {
        e.ty = Some(ty);
    }
    walk_mut(e, &mut |c| stamp_field_types(c, field_types));
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
