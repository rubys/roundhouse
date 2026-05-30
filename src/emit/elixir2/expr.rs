//! IR method body → Elixir.
//!
//! Phase 1: every body is a stub (`raise "elixir2 stub"`). This is the
//! per-variant widening point — Phase 2 grows a real `Expr`/`Stmt`
//! walker here, one variant at a time, as `ELIXIR_RUNTIME` adds files.
//! Lift idioms from the legacy `src/emit/elixir/expr.rs` and
//! `model.rs` (ivar read → `record.field`, ivar write →
//! `%{record | field: v}`, `self.foo = x` threading through returns).

use crate::dialect::MethodDef;

/// Phase 1 stub body. Valid Elixir that compiles clean under
/// `--warnings-as-errors` (no unused locals — params are `_`-prefixed
/// by the caller in `library.rs`). Replaced per-variant in Phase 2.
pub(super) fn emit_body(_m: &MethodDef) -> String {
    "raise \"elixir2 stub\"".to_string()
}
