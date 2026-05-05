//! Crystal emitter — Spinel emit shape with type annotations.
//!
//! Mirrors `src/emit/ruby.rs`'s `emit_spinel` pipeline: every artifact
//! (models, controllers, views, routes, schema, importmap, fixtures,
//! tests) is lowered to a `LibraryClass` (or LibraryFunction module),
//! then rendered via the Crystal `library` walker.
//!
//! Crystal divergences from Spinel are localized to:
//!   - File extension `.cr` and require keyword (`require "./x"`).
//!   - Method signatures carry type annotations from
//!     `MethodDef.signature` when fully typed; `Untyped` participants
//!     fall back to Crystal inference.
//!   - Hash literals render as Spinel does (`key: value` shorthand);
//!     Crystal interprets that as NamedTuple syntax, which interoperates
//!     with `**opts` parameters in helper signatures.
//!
//! The transpiled framework runtime (transpiled from `runtime/ruby/`)
//! plugs in via `src/runtime_loader.rs`'s `crystal_units` —
//! `Inflector`, `ViewHelpers`, `ActiveRecord::Base`, `ActionController::
//! Base`, etc. emit alongside the application code at app-emit time.

use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::dialect::MethodDef;

mod expr;
mod library;
mod method;
mod shared;
mod ty;

// Entry points consumed by `runtime_loader::crystal_units`. Crystal-
// side `TargetEmit` plugs these into the shared transpile driver.
pub use expr::emit_expr_for_runtime;
pub use library::{emit_library_class, emit_module};

/// Emit a single `MethodDef` as Crystal source (trailing newline
/// included). Used by the runtime-extraction pipeline.
pub fn emit_method(m: &MethodDef) -> String {
    method::emit_method(m)
}

/// Emit a full Crystal project for `app`. Composes the lowered-IR
/// emit pipeline (mirrors Spinel's `emit_spinel`).
pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    // Schema → src/schema.cr (LibraryFunction module).
    let schema_funcs = crate::lower::lower_schema_to_library_functions(&app.schema);
    if !schema_funcs.is_empty() {
        files.push(library::emit_module_file(
            &schema_funcs,
            app,
            PathBuf::from("src/schema.cr"),
        ));
    }

    // Routes dispatch table → src/routes.cr (LibraryFunction module).
    let routes_funcs = crate::lower::lower_routes_to_dispatch_functions(app);
    if !routes_funcs.is_empty() {
        files.push(library::emit_module_file(
            &routes_funcs,
            app,
            PathBuf::from("src/routes.cr"),
        ));
    }

    // Importmap → src/importmap.cr.
    let importmap_funcs = crate::lower::lower_importmap_to_library_functions(app);
    if !importmap_funcs.is_empty() {
        files.push(library::emit_module_file(
            &importmap_funcs,
            app,
            PathBuf::from("src/importmap.cr"),
        ));
    }

    // Models → src/models/<stem>.cr (one per LibraryClass, including
    // synthesized siblings like `<Model>Row`).
    let params_specs_full =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    let params_specs: std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>> =
        params_specs_full
            .iter()
            .map(|(r, s)| (r.clone(), s.fields.clone()))
            .collect();

    let model_lcs = crate::lower::lower_models_to_library_classes_with_params(
        &app.models,
        &app.schema,
        Vec::new(),
        &params_specs,
    );

    let mut synthesized_siblings: Vec<(String, String)> = model_lcs
        .iter()
        .filter(|lc| lc.origin.is_some())
        .map(|lc| {
            let name = lc.name.0.as_str().to_string();
            let stem = crate::naming::snake_case(&name);
            (name, format!("src/models/{stem}"))
        })
        .collect();
    for spec in params_specs_full.values() {
        let name = spec.class_id.0.as_str().to_string();
        let stem = crate::naming::snake_case(&name);
        synthesized_siblings.push((name, format!("src/models/{stem}")));
    }

    for lc in &model_lcs {
        let stem = crate::naming::snake_case(lc.name.0.as_str());
        let out_path = PathBuf::from(format!("src/models/{stem}.cr"));
        files.push(library::emit_library_class_decl_with_synthesized(
            lc,
            app,
            out_path,
            &synthesized_siblings,
        ));
    }

    // Controllers → src/controllers/<stem>.cr; synthesized
    // `<Resource>Params` siblings route to src/models/.
    let controller_lcs =
        crate::lower::lower_controllers_to_library_classes(&app.controllers, Vec::new());
    let controller_synth: Vec<(String, String)> = controller_lcs
        .iter()
        .filter(|lc| lc.origin.is_some())
        .map(|lc| {
            let name = lc.name.0.as_str().to_string();
            let stem = crate::naming::snake_case(&name);
            (name, format!("src/models/{stem}"))
        })
        .collect();
    for lc in &controller_lcs {
        let stem = crate::naming::snake_case(lc.name.0.as_str());
        let out_path = if lc.origin.is_some() {
            PathBuf::from(format!("src/models/{stem}.cr"))
        } else {
            PathBuf::from(format!("src/controllers/{stem}.cr"))
        };
        files.push(library::emit_library_class_decl_with_synthesized(
            lc,
            app,
            out_path,
            &controller_synth,
        ));
    }

    // Views → src/views/<dir>/<base>.cr (one per template; partials
    // keep their leading underscore).
    for v in &app.views {
        let lc = crate::lower::lower_view_to_library_class(v, app);
        let out_path = view_output_path(v.name.as_str());
        files.push(library::emit_library_class_decl(&lc, app, out_path));
    }

    // RouteHelpers → src/route_helpers.cr.
    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    if !route_helper_funcs.is_empty() {
        files.push(library::emit_module_file(
            &route_helper_funcs,
            app,
            PathBuf::from("src/route_helpers.cr"),
        ));
    }

    // Seeds → src/seeds.cr.
    let seeds_funcs = crate::lower::lower_seeds_to_library_functions(app);
    if !seeds_funcs.is_empty() {
        files.push(library::emit_module_file(
            &seeds_funcs,
            app,
            PathBuf::from("src/seeds.cr"),
        ));
    }

    files
}

/// Map a view name (`articles/index`, `articles/_article`,
/// `layouts/application`) to the Crystal output path under `src/views/`.
fn view_output_path(view_name: &str) -> PathBuf {
    PathBuf::from(format!("src/views/{view_name}.cr"))
}
