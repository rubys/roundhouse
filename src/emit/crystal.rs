//! Crystal emitter — under reconstruction.
//!
//! The legacy walker-style emitter (16 submodules, ~6,600 LOC) was removed
//! in favor of a Spinel-shape rebuild: LibraryClass-uniform consumption +
//! transpiled framework runtime via `runtime_loader::crystal_units`.
//! See memory: project_crystal_rip_and_replace.md.
//!
//! This stub keeps the public API alive (`emit`, `emit_method`) so the
//! rest of the workspace builds while the new emitter lands. Both
//! functions return empty/placeholder values; downstream Crystal CI is
//! disabled until the rebuild is in.

use super::EmittedFile;
use crate::App;
use crate::dialect::MethodDef;

mod expr;
mod library;

pub use expr::emit_expr_for_runtime;
pub use library::{emit_library_class, emit_module};

/// Emit a Crystal project for `app`. Currently returns no files —
/// the rebuild is in progress.
pub fn emit(_app: &App) -> Vec<EmittedFile> {
    Vec::new()
}

/// Emit a single `MethodDef` as Crystal source. Used by the
/// runtime-extraction pipeline (`runtime_loader::crystal_units`).
/// Currently returns an empty string — the rebuild is in progress.
pub fn emit_method(_m: &MethodDef) -> String {
    String::new()
}
