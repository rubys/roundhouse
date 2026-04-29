//! TypeScript emitter — STUB during rip-and-replace migration.
//!
//! The previous emitter (~6.2k lines across src/emit/typescript/*.rs)
//! was ripped on 2026-04-28 to be rebuilt slice-by-slice against the
//! spinel-blog canonical output shape. See
//! project_emitter_rip_and_replace memory.
//!
//! Each public fn keeps its signature so the rest of the crate still
//! compiles (build-site, emit_preview, runtime_src pipeline) and
//! returns empty/placeholder values. The 32 ignored TS tests under
//! tests/ are the re-entry gate; remove `#[ignore]` from each as the
//! corresponding slice lands.

use super::EmittedFile;
use crate::App;

pub fn emit(_app: &App) -> Vec<EmittedFile> {
    Vec::new()
}

pub fn emit_library(_app: &App) -> Vec<EmittedFile> {
    Vec::new()
}

pub fn emit_with_adapter(
    _app: &App,
    _adapter: &dyn crate::adapter::DatabaseAdapter,
) -> Vec<EmittedFile> {
    Vec::new()
}

pub fn emit_method(_m: &crate::dialect::MethodDef) -> String {
    String::new()
}
