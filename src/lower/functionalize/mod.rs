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

pub mod mutation_to_struct_return;
pub mod while_to_recursion;

use crate::dialect::LibraryClass;

/// Apply the functional-lowering pass family to a set of library
/// classes. Called by functional emitters only. Each method may be
/// rewritten in place or expanded into several methods (e.g. a loop
/// method → an entry + a recursive helper).
pub fn functionalize(classes: Vec<LibraryClass>) -> Vec<LibraryClass> {
    classes
        .into_iter()
        .map(|mut class| {
            let methods = std::mem::take(&mut class.methods);
            class.methods = methods
                .into_iter()
                // while→recursion first (may split a method into entry +
                // helper), then thread instance mutation through the
                // results.
                .flat_map(while_to_recursion::transform_method)
                .map(mutation_to_struct_return::transform_method)
                .collect();
            class
        })
        .collect()
}
