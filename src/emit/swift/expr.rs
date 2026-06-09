//! `Expr` → Swift source.
//!
//! Phase 1 skeleton — empty. Phase 2 ports the IR expression walker here,
//! from `src/emit/kotlin/expr.rs` (the template — same walker shape, with
//! the Swift deltas: `\(...)` interpolation, `!` not `!!`, `try` at
//! throwing call sites). Until then `swift::emit` produces only the SPM
//! scaffold, so nothing references this module yet.
