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

    // Transpiled framework runtime — `runtime/ruby/*.rb` files
    // converted to Crystal at app emit time. Each unit lands at the
    // path declared in `CRYSTAL_RUNTIME` (e.g. `src/inflector.cr`).
    // Identity transform on the parsed classes for now; tree-shake
    // can drop unreachable methods in a follow-up pass.
    let runtime_units = crate::runtime_loader::crystal_units(|_path, classes| classes)
        .expect("crystal runtime transpile failed (Ruby source error)");
    for unit in runtime_units {
        files.push(EmittedFile {
            path: unit.out_path,
            content: unit.content,
        });
    }

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

    // src/app.cr — aggregator that requires every emitted .cr file
    // (plus the upcoming primitive runtime layer). Crystal's compiler
    // does whole-program analysis after parsing all sources, so the
    // require ordering doesn't have to follow the dependency graph;
    // we sort alphabetically for determinism.
    files.push(emit_app_cr(&files));

    files
}

/// Emit `src/app.cr` — the aggregator entry point. Requires every
/// emitted Crystal file. Loaded in dependency order: framework
/// runtime first (with explicit ordering for `include` resolution),
/// then app code (alphabetical).
///
/// Crystal processes `include` at parse time — a file with `class X
/// include Y` needs `Y`'s module loaded BEFORE the include line is
/// parsed. Whole-program analysis can't help here. The framework
/// runtime has known dependencies (active_record_base includes
/// Validations; action_controller_base may include other modules);
/// hardcoding the order keeps things deterministic.
fn emit_app_cr(emitted: &[EmittedFile]) -> EmittedFile {
    // Framework runtime files in dependency order. Each must be
    // loaded before any file that `include`s its module.
    const RUNTIME_ORDER: &[&str] = &[
        "inflector",
        "errors",
        "validations",
        "active_record_base",
        "parameters",
        "router",
        "action_controller_base",
        "view_helpers",
    ];

    let mut all_stems: Vec<String> = emitted
        .iter()
        .filter_map(|f| {
            let p = f.path.to_string_lossy();
            let s = p.strip_prefix("src/")?;
            let stem = s.strip_suffix(".cr")?;
            Some(stem.to_string())
        })
        .collect();
    all_stems.sort();
    all_stems.dedup();

    let mut ordered: Vec<String> = Vec::new();
    // Runtime in declared order.
    for name in RUNTIME_ORDER {
        if all_stems.iter().any(|s| s == *name) {
            ordered.push((*name).to_string());
        }
    }
    // Everything else alphabetical.
    for stem in &all_stems {
        if !RUNTIME_ORDER.contains(&stem.as_str()) {
            ordered.push(stem.clone());
        }
    }

    let mut s = String::new();
    s.push_str("# Generated by Roundhouse — Crystal entry point.\n");
    s.push_str("# Requires the framework runtime in dependency order\n");
    s.push_str("# (Crystal processes `include` at parse time, so modules\n");
    s.push_str("# included by classes must be loaded first), then app code\n");
    s.push_str("# alphabetically.\n\n");
    for p in &ordered {
        s.push_str(&format!("require \"./{p}\"\n"));
    }

    EmittedFile {
        path: PathBuf::from("src/app.cr"),
        content: s,
    }
}

/// Map a view name (`articles/index`, `articles/_article`,
/// `layouts/application`) to the Crystal output path under `src/views/`.
fn view_output_path(view_name: &str) -> PathBuf {
    PathBuf::from(format!("src/views/{view_name}.cr"))
}
