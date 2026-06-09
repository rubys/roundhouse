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
mod naming;
mod package;
mod ty;

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    // SPM scaffold (Package.swift, the CSQLite systemLibrary target,
    // .gitignore).
    files.extend(package::scaffold());

    // Models → Sources/App/app/models/<Name>.swift. Same lowering recipe
    // as crystal/kotlin: a preliminary view pass seeds the class-info
    // registry, then models lower to LibraryClasses (including
    // synthesized `<Model>Row` siblings).
    let preliminary_views: Vec<crate::dialect::LibraryClass> = app
        .views
        .iter()
        .map(|v| crate::lower::lower_view_to_library_class(v, app))
        .collect();
    let view_extras = crate::lower::extras_from_lcs(&preliminary_views);
    let (model_lcs, _registry) =
        crate::lower::lower_models_with_registry(&app.models, &app.schema, view_extras);
    library::register_classes(&model_lcs);
    for lc in &model_lcs {
        files.push(library::emit_class_file(lc));
    }

    // Phase 3+: transpiled framework runtime + hand-written primitives +
    // controllers/views.
    files
}
