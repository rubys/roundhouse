//! Explicit type-coercion insertion across LibraryClass method bodies.
//!
//! Walks each Send in each method body and, where a positional arg's
//! `Ty` is narrower than the callee's declared param `Ty`, wraps the
//! arg in `ExprNode::Cast { value, target_ty }`. Downstream emitters
//! consume the Cast nodes per-target — rust2 widens
//! `HashMap<K,V>` via `into_iter().map().collect()`, go2 produces a
//! `map[string]any` conversion, TS/Crystal/Ruby treat Cast as identity
//! (their typers handle widening natively, so the Cast node is a
//! pass-through).
//!
//! This replaces emit-time back-propagation that derives the same
//! information from arg-vs-param Ty comparisons at every call site in
//! every emitter. Landing the typing intent once in the IR means each
//! emitter just consumes a uniform construct.
//!
//! Stage 1 (this file): scaffolding only — function exists, is wired
//! into the emit pipelines, but inserts no Cast nodes. Validates the
//! integration point with zero behavior change. Subsequent stages add
//! coercion families:
//!   - Hash widening (the form_attrs/render_attrs go2 blocker case)
//!   - T → Option<T> Some-wrap (rust2 Family 6)
//!   - Symbol → String key rewrites

use crate::dialect::LibraryClass;

/// Insert `ExprNode::Cast` wrappers at call-site arg positions where
/// the callee's declared param Ty differs from the arg's Ty in a way
/// that needs an explicit coercion. Mutates `lcs` in place.
///
/// Stage 1: no-op. The pass body is empty so this lands additively
/// without behavior change. Subsequent stages populate it.
pub fn insert_ty_coercions(_lcs: &mut [LibraryClass]) {}
