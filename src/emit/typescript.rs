//! TypeScript emitter — rebuild in progress.
//!
//! Being rebuilt slice-by-slice against the spinel-blog canonical
//! output shape (see project_emitter_rip_and_replace memory). Each
//! commit lands one slice; the 32 ignored TS tests under tests/ are
//! the re-entry gate.
//!
//! Slice 1 (this revision): package.json + main.ts.

use super::EmittedFile;
use crate::App;

mod main_ts;
mod model;
mod package;
mod ty;

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(package::emit_package_json());
    files.push(main_ts::emit_main_ts(app));
    files.extend(model::emit_models(app));
    files
}

pub fn emit_library(_app: &App) -> Vec<EmittedFile> {
    Vec::new()
}

pub fn emit_with_adapter(
    app: &App,
    _adapter: &dyn crate::adapter::DatabaseAdapter,
) -> Vec<EmittedFile> {
    emit(app)
}

pub fn emit_method(_m: &crate::dialect::MethodDef) -> String {
    String::new()
}
