//! Swift emitter — backend-only target (see
//! `docs/swift-migration-plan.md`).
//!
//! Lowerer-first, like every roundhouse target: the Rails DSL is already
//! lowered to the universal post-lowering IR; this emitter renders it to
//! Swift. The Kotlin emitter (`src/emit/kotlin/`) is the near-exact
//! template — same modern-OO profile, same *soft* strict-typing posture
//! (`Ty::Untyped → Any?` with no emit diagnostic, see `ty.rs`); the
//! genuine deltas are checked `throws`, `object` → caseless `enum`, and
//! value-type collections.
//!
//! **Phase 1 (this commit): skeleton.** `emit` produces only the SPM
//! scaffold (`package::scaffold`). The `ty` mapping is complete; `expr`
//! and `library` are empty skeletons. Phase 2 adds model emit → swiftc
//! clean; Phase 3 controllers + the transpiled framework runtime; Phase 4
//! HTML-string views. The hand-written reference the emitter is driven
//! toward lives in `swift-reference/`.

use super::EmittedFile;
use crate::App;

mod expr;
mod library;
mod package;
mod ty;

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    // SPM scaffold (Package.swift, the CSQLite systemLibrary target,
    // .gitignore).
    files.extend(package::scaffold());

    // Phase 2+: transpiled framework runtime + hand-written primitives +
    // models/controllers/views lowered to Swift source under
    // `Sources/App/`. Not yet wired — the app is unused for now.
    let _ = app;

    files
}
