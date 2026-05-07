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

/// Minimal `shard.yml` for the emitted project. Crystal's package
/// manager (shards) requires this to resolve dependencies. sqlite3
/// is the only external dep; HTTP / WebSocket / Spec come from
/// Crystal's stdlib.
const SHARD_YML: &str = "name: roundhouse-app
version: 0.1.0
crystal: \">= 1.6.0\"

dependencies:
  sqlite3:
    github: crystal-lang/crystal-sqlite3
";

// Hand-written Crystal primitive runtime — copied verbatim into the
// generated project as `src/<file>.cr`. These provide HTTP / DB /
// cable / spec-helper glue that the transpiled framework runtime
// stands on top of. Ruby-shape framework code (ActionController::Base,
// ActiveRecord::Base, Router, ViewHelpers) lives in `runtime/ruby/`
// and transpiles via `runtime_loader::crystal_units`.
const CR_DB_SOURCE: &str = include_str!("../../runtime/crystal/db.cr");
const CR_HTTP_SOURCE: &str = include_str!("../../runtime/crystal/http.cr");
const CR_SERVER_SOURCE: &str = include_str!("../../runtime/crystal/server.cr");
const CR_CABLE_SOURCE: &str = include_str!("../../runtime/crystal/cable.cr");
const CR_TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/crystal/test_support.cr");
const CR_BROADCASTS_SOURCE: &str = include_str!("../../runtime/crystal/broadcasts.cr");

/// Emit a full Crystal project for `app`. Composes the lowered-IR
/// emit pipeline (mirrors Spinel's `emit_spinel`).
pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    // shard.yml — minimal Crystal project manifest with sqlite3
    // dependency (db.cr's only external requirement).
    files.push(EmittedFile {
        path: PathBuf::from("shard.yml"),
        content: SHARD_YML.to_string(),
    });

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

    // Primitive runtime — hand-written platform glue.
    files.push(EmittedFile {
        path: PathBuf::from("src/db.cr"),
        content: CR_DB_SOURCE.to_string(),
    });
    files.push(EmittedFile {
        path: PathBuf::from("src/http.cr"),
        content: CR_HTTP_SOURCE.to_string(),
    });
    files.push(EmittedFile {
        path: PathBuf::from("src/server.cr"),
        content: CR_SERVER_SOURCE.to_string(),
    });
    files.push(EmittedFile {
        path: PathBuf::from("src/cable.cr"),
        content: CR_CABLE_SOURCE.to_string(),
    });
    files.push(EmittedFile {
        path: PathBuf::from("src/test_support.cr"),
        content: CR_TEST_SUPPORT_SOURCE.to_string(),
    });
    files.push(EmittedFile {
        path: PathBuf::from("src/broadcasts.cr"),
        content: CR_BROADCASTS_SOURCE.to_string(),
    });

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
    // synthesized siblings like `<Model>Row`). Mirrors the TS pipeline:
    // a preliminary view lowering seeds the model registry's
    // `Views::*` entries; the model lowerer returns a registry the
    // controller/view lowerers extend so cross-class dispatch
    // (`Article.find(...)`, `Views::Articles.index(...)`) types
    // through.
    let params_specs_full =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    let params_specs: std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>> =
        params_specs_full
            .iter()
            .map(|(r, s)| (r.clone(), s.fields.clone()))
            .collect();

    let preliminary_views: Vec<crate::dialect::LibraryClass> = app
        .views
        .iter()
        .map(|v| crate::lower::lower_view_to_library_class(v, app))
        .collect();
    let view_extras = crate::lower::extras_from_lcs(&preliminary_views);

    let route_helper_funcs_for_extras = crate::lower::lower_routes_to_library_functions(app);
    let route_helper_extras = crate::lower::extras_from_funcs(&route_helper_funcs_for_extras);

    let (model_lcs, model_registry) = crate::lower::lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        view_extras,
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

    // Synthesize `ApplicationRecord` if any emitted model references
    // it as parent but the app doesn't define one. Rails scaffolds
    // make this base class explicit; minimal fixtures sometimes skip
    // it. Crystal needs the class defined for the parent reference
    // to resolve.
    let needs_app_record = model_lcs
        .iter()
        .any(|lc| matches!(lc.parent.as_ref(), Some(p) if p.0.as_str() == "ApplicationRecord"))
        && !model_lcs
            .iter()
            .any(|lc| lc.name.0.as_str() == "ApplicationRecord");
    if needs_app_record {
        files.push(EmittedFile {
            path: PathBuf::from("src/models/application_record.cr"),
            content: "class ApplicationRecord < ActiveRecord::Base\nend\n".to_string(),
        });
    }

    // Synthesize `ApplicationController` if any emitted controller
    // references it as parent but the app doesn't define one. Rails
    // scaffolds always extend `ApplicationController`; minimal
    // fixtures (tiny-blog) skip the explicit declaration. Without
    // this, Crystal compilation fails on the dangling reference.
    let needs_app_controller = app
        .controllers
        .iter()
        .any(|c| matches!(c.parent.as_ref(), Some(p) if p.0.as_str() == "ApplicationController"))
        && !app
            .controllers
            .iter()
            .any(|c| c.name.0.as_str() == "ApplicationController");
    if needs_app_controller {
        files.push(EmittedFile {
            path: PathBuf::from("src/controllers/application_controller.cr"),
            content: "class ApplicationController < ActionController::Base\nend\n".to_string(),
        });
    }

    // Re-lower views with the model registry so view bodies dispatch
    // models correctly (`@article.title` etc.). Used both to emit the
    // view classes (below) AND to feed the controller lowerer's
    // class registry.
    let mut view_lower_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.clone().into_iter().collect();
    view_lower_extras.extend(route_helper_extras.clone());
    let view_lcs =
        crate::lower::lower_views_to_library_classes(&app.views, app, view_lower_extras);

    // Controllers → src/controllers/<stem>.cr; synthesized
    // `<Resource>Params` siblings route to src/models/.
    let mut controller_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    controller_extras.extend(crate::lower::extras_from_lcs(&view_lcs));
    controller_extras.extend(route_helper_extras);
    let controller_lcs =
        crate::lower::lower_controllers_to_library_classes(&app.controllers, controller_extras);
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
    // keep their leading underscore). `view_lcs` was lowered above
    // with the model registry so each view body's model dispatches
    // type. Pair them with the corresponding `View` so we get the
    // right output path.
    for (v, lc) in app.views.iter().zip(view_lcs.iter()) {
        let out_path = view_output_path(v.name.as_str());
        files.push(library::emit_library_class_decl(lc, app, out_path));
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

    // src/main.cr — the binary entry point. `scripts/compare crystal`
    // builds this; `crystal_toolchain` tests target src/app.cr (just
    // compile check). main.cr requires app.cr and boots the server
    // primitive — the actual dispatcher lives in runtime/crystal/
    // server.cr (now src/server.cr after emit).
    files.push(emit_main_cr(app));

    files
}

/// Emit `src/main.cr` — the binary entry point. Wires together the
/// schema DDL, routes table, and controller registry, then hands them
/// to `Roundhouse::Server.start`. Mirrors the TS emit_main_ts pipeline.
fn emit_main_cr(app: &App) -> EmittedFile {
    use std::fmt::Write;
    let has_routes = !crate::lower::routes::flatten_routes(app).is_empty();
    let has_root = crate::lower::routes::flatten_routes(app)
        .iter()
        .any(|r| r.path == "/");
    let has_layout = app.views.iter().any(|v| v.name.as_str() == "layouts/application");

    let mut s = String::new();
    s.push_str("# Generated by Roundhouse — Crystal binary entry point.\n");
    s.push_str("require \"./app\"\n\n");

    s.push_str("Roundhouse::Server.start(\n");
    s.push_str("  schema_sql: Schema.statements.join(\";\\n\"),\n");
    if has_routes {
        s.push_str("  routes: Routes.table,\n");
    } else {
        s.push_str("  routes: [] of NamedTuple(method: String, pattern: String, controller: Symbol, action: Symbol),\n");
    }
    if has_root {
        s.push_str("  root_route: Routes.root,\n");
    }
    if has_layout {
        // Wire the transpiled layout method as the per-response wrapper.
        // Matches TS's main.ts pattern (server passes the body to a
        // user-provided proc that returns the wrapped HTML). Without
        // this, responses ship without `<!DOCTYPE html>` / `<html>` /
        // `<body>` and the cross-target compare DOM-diff fails.
        s.push_str("  layout: ->(body : String) { Views::Layouts.application(body) },\n");
    }
    s.push_str("  controllers: {\n");
    for c in &app.controllers {
        let class_name = c.name.0.as_str();
        let stem = crate::naming::snake_case(
            class_name.strip_suffix("Controller").unwrap_or(class_name),
        );
        writeln!(s, "    :{} => {},", stem, class_name).unwrap();
    }
    s.push_str("  } of Symbol => ActionController::Base.class,\n");
    s.push_str(")\n");
    EmittedFile {
        path: PathBuf::from("src/main.cr"),
        content: s,
    }
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
        "hash_with_indifferent_access",
        "inflector",
        // validations → active_record_base → errors keeps the
        // forward-ref chain resolvable at parse time. Validations
        // module is included by Base (parse-time `include`); Base is
        // referenced by errors' RecordInvalid as a property type
        // (parse-time class macro); errors is used by Base's body
        // `raise RecordNotFound/RecordInvalid` (lazy method-body
        // resolution).
        "validations",
        "active_record_base",
        "errors",
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
