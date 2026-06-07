//! Kotlin emitter — backend-only target (see
//! `docs/kotlin-migration-plan.md`).
//!
//! Lowerer-first, like every roundhouse target: the Rails DSL is already
//! lowered to the universal post-lowering IR; this emitter renders it to
//! Kotlin. The TypeScript emitter is the structural template (modern OO,
//! generics, declared nullability); the divergence is Kotlin's static
//! type system, softened by the `Ty::Untyped → Any?` escape hatch (see
//! `ty.rs`).
//!
//! **Phase 1 (this commit): skeleton.** `emit` produces only the Gradle
//! scaffold (`package::scaffold`). The `ty` mapping is complete; `expr`
//! and `library` are empty skeletons. Phase 2 adds model emit → kotlinc
//! clean; Phase 3 controllers + the transpiled framework runtime; Phase 4
//! HTML-string views. The hand-written reference the emitter is driven
//! toward lives in `kotlin-reference/`.

use super::EmittedFile;
use crate::App;

mod expr;
mod library;
mod naming;
mod package;
mod ty;

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    // Gradle scaffold (build.gradle.kts, settings.gradle.kts, .gitignore).
    files.extend(package::scaffold());

    // Models → src/main/kotlin/app/models/<Name>.kt. Same lowering recipe
    // as crystal/dump_ir: a preliminary view pass seeds the class-info
    // registry, then models lower to LibraryClasses (including synthesized
    // `<Model>Row` siblings).
    let preliminary_views: Vec<crate::dialect::LibraryClass> = app
        .views
        .iter()
        .map(|v| crate::lower::lower_view_to_library_class(v, app))
        .collect();
    let view_extras = crate::lower::extras_from_lcs(&preliminary_views);
    let (model_lcs, _registry) =
        crate::lower::lower_models_with_registry(&app.models, &app.schema, view_extras);
    for lc in &model_lcs {
        files.push(library::emit_class_file(lc));
    }

    // Phase 3+: transpiled framework runtime + hand-written primitives +
    // controllers/views.
    files
}
