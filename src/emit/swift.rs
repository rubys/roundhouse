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
mod primitives;
mod ty;

// Entry points consumed by `runtime_loader::swift_units`.
pub use expr::emit_constant_for_runtime;
pub use library::{emit_library_class_result, emit_module};

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    // SPM scaffold (Package.swift, the CSQLite systemLibrary target,
    // .gitignore).
    files.extend(package::scaffold());

    // Hand-written primitives (`Sources/App/runtime/`).
    files.extend(primitives::primitives());

    // Transpiled framework runtime — `runtime/ruby/*.rb` → Swift under
    // `Sources/App/`. Grown one file at a time (Phase 3). The transform
    // closure pre-registers each entry's classes (parents, Error
    // conformance, throwing methods, object accessors) before that entry
    // renders, so call sites resolve regardless of order.
    expr::reset_registries();
    let runtime_units = crate::runtime_loader::swift_units(|_path, classes| {
        library::register_classes(&classes);
        classes
    })
    .expect("swift runtime transpile failed (Ruby source error)");
    for unit in runtime_units {
        files.push(EmittedFile { path: unit.out_path, content: unit.content });
    }

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
    let (model_lcs, model_registry) =
        crate::lower::lower_models_with_registry(&app.models, &app.schema, view_extras);
    library::register_classes(&model_lcs);
    for lc in &model_lcs {
        files.push(library::emit_class_file(lc));
    }

    // Views → Sources/App/app/views/<Name>.swift. Each ERB template
    // lowers to its own `Views::<Plural>` LibraryClass carrying one
    // render method; re-lower here with the model registry (+ route
    // helper / importmap extras) so view bodies dispatch model
    // attributes and route helpers type. Swift enums can't be reopened
    // (same as Kotlin objects), so templates sharing a module merge into
    // one `enum Articles { ... }` — the name the broadcast callbacks and
    // controllers call (`Articles.article(self)`).
    let mut view_lower_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    view_lower_extras.extend(crate::lower::extras_from_funcs(&route_helper_funcs));
    if let Some(f) = library::emit_function_module(&route_helper_funcs) {
        files.push(f);
    }
    // Importmap pins/entry → `enum Importmap` (the layout's
    // `javascript_importmap_tags` lowers to `Importmap.pins()`/`.entry()`).
    let importmap_funcs = crate::lower::lower_importmap_to_library_functions(app);
    view_lower_extras.extend(crate::lower::extras_from_funcs(&importmap_funcs));
    if let Some(f) = library::emit_function_module(&importmap_funcs) {
        files.push(f);
    }
    let view_lcs =
        crate::lower::lower_views_to_library_classes(&app.views, app, view_lower_extras.clone());
    // Jbuilder (json-format) views lower to `<name>_json` methods on the
    // same `Views::<Plural>` module; merge them into the html view enums
    // so a controller's JSON branch resolves `Articles.indexJson(...)`.
    let jbuilder_lcs = crate::lower::lower_jbuilder_to_library_classes(
        &app.views,
        app,
        view_lower_extras.clone(),
    );
    let mut all_view_lcs = view_lcs;
    all_view_lcs.extend(jbuilder_lcs);
    for lc in merge_by_module(all_view_lcs) {
        let last = lc.name.0.as_str().rsplit("::").next().unwrap_or(lc.name.0.as_str());
        files.push(EmittedFile {
            path: std::path::PathBuf::from(format!("Sources/App/app/views/{last}.swift")),
            content: format!("import Foundation\n\n{}", library::emit_library_class(&lc)),
        });
    }

    // Phase 5+: controllers + Server/Main.
    files
}

/// Swift enums can't be reopened — templates sharing a `Views::<Plural>`
/// module collapse into one LibraryClass (concat methods, first-seen
/// order).
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
