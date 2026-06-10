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
    // `processAction` is the dispatch boundary the server catches at —
    // its Base declaration carries `throws` by contract so the throwing
    // controller overrides are legal.
    library::register_throws_contract("ActionControllerBase", "processAction");
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
    // Permitted-params specs (resource → fields) collected from the
    // controllers, so each model gains a typed `fromParams(<Model>Params)`
    // factory the controllers call.
    let params_specs_full =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    let params_specs: std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>> =
        params_specs_full.iter().map(|(r, s)| (r.clone(), s.fields.clone())).collect();
    let (model_lcs, model_registry) = crate::lower::lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        view_extras,
        &params_specs,
    );
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
    let mut all_view_lcs = view_lcs.clone();
    all_view_lcs.extend(jbuilder_lcs);
    let merged_views = merge_by_module(all_view_lcs);
    library::register_classes(&merged_views);
    for lc in &merged_views {
        let last = lc.name.0.as_str().rsplit("::").next().unwrap_or(lc.name.0.as_str());
        files.push(EmittedFile {
            path: std::path::PathBuf::from(format!("Sources/App/app/views/{last}.swift")),
            content: format!("import Foundation\n\n{}", library::emit_library_class(lc)),
        });
    }

    // Controllers → Sources/App/app/controllers/<Name>.swift. Lowered
    // with the full registry (models + routes + importmap + views) so
    // action bodies dispatch `Article.all`, `Articles.index(...)`, route
    // helpers, etc. Synthesized `<Resource>Params` siblings
    // (origin-tagged) route to app/models alongside the model classes.
    let mut controller_extras = view_lower_extras;
    controller_extras.extend(crate::lower::extras_from_lcs(&view_lcs));
    let assocs = crate::lower::model_associations::compute_association_graph(app);
    let controller_lcs = crate::lower::lower_controllers_with_arel_views_and_assocs(
        &app.controllers,
        controller_extras,
        Some(&app.schema),
        &app.views,
        &assocs,
    );
    // Synthesize `ApplicationController` when a controller extends it but
    // the app doesn't define one.
    let needs_app_controller = app
        .controllers
        .iter()
        .any(|c| matches!(c.parent.as_ref(), Some(p) if p.0.as_str() == "ApplicationController"))
        && !app.controllers.iter().any(|c| c.name.0.as_str() == "ApplicationController");
    if needs_app_controller {
        files.push(EmittedFile {
            path: std::path::PathBuf::from(
                "Sources/App/app/controllers/ApplicationController.swift",
            ),
            content: "class ApplicationController: ActionControllerBase {\n}\n".to_string(),
        });
        library::register_synthetic_class("ApplicationController", "ActionControllerBase");
    }
    library::register_classes(&controller_lcs);
    for lc in &controller_lcs {
        let last = lc.name.0.as_str().rsplit("::").next().unwrap_or(lc.name.0.as_str());
        let dir = if lc.origin.is_some() { "models" } else { "controllers" };
        files.push(EmittedFile {
            path: std::path::PathBuf::from(format!("Sources/App/app/{dir}/{last}.swift")),
            content: format!("import Foundation\n\n{}", library::emit_library_class(lc)),
        });
    }

    // main.swift — the entrypoint: builds the routes table + controller
    // factory map (app-specific) and hands them to the Server primitive.
    files.push(emit_main(app));

    files
}

/// The generated entrypoint. `BLOG_DB`/`DATABASE_PATH`/`PORT` mirror the
/// env every other target reads in scripts/compare + scripts/bench.
/// (Named `main.swift` — top-level await is only legal there.)
fn emit_main(app: &App) -> EmittedFile {
    use crate::dialect::HttpMethod;
    let routes = crate::lower::flatten_routes(app);
    let verb = |m: &HttpMethod| match m {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Head => "HEAD",
        HttpMethod::Options => "OPTIONS",
        HttpMethod::Any => "GET",
    };
    let route_lines: Vec<String> = routes
        .iter()
        .map(|r| {
            format!(
                "    Route({:?}, {:?}, {:?}, {:?}),",
                verb(&r.method),
                r.path,
                r.controller.0.as_str(),
                r.action.as_str(),
            )
        })
        .collect();

    let mut controllers: Vec<String> =
        routes.iter().map(|r| r.controller.0.as_str().to_string()).collect();
    controllers.sort();
    controllers.dedup();
    let ctrl_lines: Vec<String> =
        controllers.iter().map(|c| format!("    {c:?}: {{ {c}() }},")).collect();

    // The layout wraps every html response; identity when the app has none.
    let has_layout = app.views.iter().any(|v| v.name.as_str() == "layouts/application");
    let layout_expr = if has_layout {
        "{ body, notice, alert in Layouts.application(body, notice, alert) }"
    } else {
        "{ body, _, _ in body }"
    };

    let content = format!(
        "// Generated by Roundhouse (swift). Entry point — wires the routes\n\
         // table + controllers into the Server primitive.\n\
         \nimport Foundation\n\
         \nlet dbPath = ProcessInfo.processInfo.environment[\"BLOG_DB\"]\n\
         \x20\x20\x20\x20?? ProcessInfo.processInfo.environment[\"DATABASE_PATH\"]\n\
         \x20\x20\x20\x20?? \"storage/development.sqlite3\"\n\
         let port = Int(ProcessInfo.processInfo.environment[\"PORT\"] ?? \"9000\") ?? 9000\n\
         let routes: [Route] = [\n{}\n]\n\
         let controllers: [String: () -> ActionControllerBase] = [\n{}\n]\n\
         try await Server.start(dbPath, port, routes, controllers, {})\n",
        route_lines.join("\n"),
        ctrl_lines.join("\n"),
        layout_expr,
    );
    EmittedFile { path: std::path::PathBuf::from("Sources/App/main.swift"), content }
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
