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
mod primitives;
mod ty;

// Entry points consumed by `runtime_loader::kotlin_units`.
pub use expr::emit_expr_for_runtime;
pub use library::{emit_library_class_result, emit_module};

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    // Gradle scaffold (build.gradle.kts, settings.gradle.kts, .gitignore).
    files.extend(package::scaffold());

    // Hand-written runtime primitives (Time, … — the JVM-bridging bottom
    // layer the transpiled runtime calls into).
    files.extend(primitives::primitives());

    // Transpiled framework runtime — `runtime/ruby/*.rb` → Kotlin under
    // `src/main/kotlin/`. Grown one file at a time (Phase 3). The transform
    // runs as a pre-scan before each entry renders: it registers
    // module/object-level accessors (e.g. `ActiveRecord.adapter`) so reads
    // of them drop their call parens.
    expr::reset_object_accessors();
    expr::reset_class_hierarchy();
    expr::reset_method_params();
    let runtime_units = crate::runtime_loader::kotlin_units(|_path, classes| {
        library::register_object_accessors(&classes);
        // Register the runtime classes (Base, …) for override resolution
        // before any model renders. Base has no parent, so its members are
        // all `open` regardless of order; subclasses look up its members
        // here when deciding which to mark `override`.
        library::register_class_hierarchy(&classes);
        classes
    })
    .expect("kotlin runtime transpile failed (Ruby source error)");
    for unit in runtime_units {
        files.push(EmittedFile { path: unit.out_path, content: unit.content });
    }

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
    let (model_lcs, model_registry) =
        crate::lower::lower_models_with_registry(&app.models, &app.schema, view_extras);
    // Register the model classes (ApplicationRecord, Article, …) before
    // rendering any of them, so a model that extends another model
    // (Article → ApplicationRecord) sees the parent's members for override
    // resolution regardless of emit order.
    library::register_class_hierarchy(&model_lcs);
    for lc in &model_lcs {
        files.push(library::emit_class_file(lc));
    }

    // Views → src/main/kotlin/app/views/<Name>.kt. Each ERB template lowers
    // to its own `Views::<Plural>` LibraryClass carrying one render method;
    // re-lower here with the model registry (+ route-helper stubs) so view
    // bodies dispatch model attributes (`article.title`) and route helpers
    // type. Kotlin `object`s can't be reopened, so templates sharing a
    // module (`Views::Articles#index` + `#article` + …) merge into one
    // `object Articles { ... }` — the name the broadcast callbacks and
    // controllers call (`Articles.article(this)`, last-segment of
    // `Views::Articles`).
    let mut view_lower_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    view_lower_extras.extend(crate::lower::extras_from_funcs(&route_helper_funcs));
    if let Some(f) = library::emit_route_helpers(&route_helper_funcs) {
        files.push(f);
    }
    let view_lcs =
        crate::lower::lower_views_to_library_classes(&app.views, app, view_lower_extras);
    for lc in merge_by_module(view_lcs) {
        let last = lc.name.0.as_str().rsplit("::").next().unwrap_or(lc.name.0.as_str());
        files.push(EmittedFile {
            path: std::path::PathBuf::from(format!("src/main/kotlin/app/views/{last}.kt")),
            content: format!("package roundhouse\n\n{}", library::emit_library_class(&lc)),
        });
    }

    // Phase 4+: hand-written ViewHelpers/RouteHelpers + controllers +
    // Server.kt (Javalin) + Main.kt.
    files
}

/// Merge `LibraryClass`es that share a module name into one (concatenating
/// their methods), preserving first-seen order. The view lowerer produces
/// one LC per template, several sharing a `Views::<Plural>` name; Kotlin
/// `object`s are closed (not reopenable across declarations) so they must
/// collapse into a single object before emit.
fn merge_by_module(lcs: Vec<crate::dialect::LibraryClass>) -> Vec<crate::dialect::LibraryClass> {
    let mut merged: Vec<crate::dialect::LibraryClass> = Vec::new();
    for lc in lcs {
        if let Some(existing) = merged.iter_mut().find(|m| m.name == lc.name) {
            existing.methods.extend(lc.methods);
        } else {
            merged.push(lc);
        }
    }
    merged
}
