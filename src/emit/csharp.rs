//! C# / .NET emitter — backend-only target (see
//! `docs/csharp-migration-plan.md`).
//!
//! Lowerer-first, like every roundhouse target: the Rails DSL is already
//! lowered to the universal post-lowering IR; this emitter renders it to
//! C#. The Kotlin emitter is the structural template (both nominal, GC'd,
//! declared nullability); see `expr.rs`/`library.rs` for the per-node
//! rendering and the C#-specific divergences (semicolons, `switch`,
//! `??`/`!`, collection literals, indexers, constructors).
//!
//! **Phase 2 (this commit): model emit.** `emit` produces the .NET scaffold,
//! the hand-written runtime primitives (`runtime/csharp/`), and the lowered
//! **models** (`Article`, `Comment`, the abstract `ApplicationRecord`, and
//! the synthesized `<Model>Row`/`<Model>Params` siblings) as `app/models/
//! *.cs`. Views are stubbed (the `after_*_commit` broadcast callbacks
//! reference view modules); controllers + the transpiled framework runtime
//! land in Phase 3. See `docs/csharp-migration-plan.md`.

use std::collections::{BTreeMap, BTreeSet};

use super::EmittedFile;
use crate::App;

mod expr;
mod library;
mod naming;
mod package;
mod primitives;
mod ty;

// Entry points consumed by `runtime_loader::csharp_units` (the framework
// runtime transpile).
pub use expr::{emit_constant_for_runtime, emit_expr_for_runtime};
pub use library::{emit_library_class_result, emit_module, emit_module_constant};

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    // .NET project scaffold (`roundhouse-app.csproj`, `Program.cs`).
    files.extend(package::scaffold());

    // Hand-written runtime primitives (the base class, Db, Time, Broadcasts,
    // errors — the .NET-bridging bottom layer the emitted models call into).
    files.extend(primitives::primitives());

    // Reset the per-emit registries.
    expr::reset_class_hierarchy();
    expr::reset_object_accessors();
    expr::reset_method_params();

    // Transpiled framework runtime — `runtime/ruby/*.rb` → C# under
    // `app/runtime/`. Grown one file at a time (Phase 3); the pre-scan
    // registers each runtime class's object accessors + hierarchy before any
    // model renders, mirroring `kotlin_units`.
    let runtime_units = crate::runtime_loader::csharp_units(|_path, classes| {
        library::register_object_accessors(&classes);
        library::register_class_hierarchy(&classes);
        classes
    })
    .expect("csharp runtime transpile failed (Ruby source error)");
    for unit in runtime_units {
        files.push(EmittedFile { path: unit.out_path, content: unit.content });
    }

    // Preliminary view pass: seeds the model lowerer's association element
    // types, and gives the view-module method names for the Phase-2 stubs.
    let preliminary_views: Vec<crate::dialect::LibraryClass> = app
        .views
        .iter()
        .map(|v| crate::lower::lower_view_to_library_class(v, app))
        .collect();
    let view_extras = crate::lower::extras_from_lcs(&preliminary_views);

    // Permitted-params specs → each model gains a typed `from_params`.
    let params_specs_full =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    let params_specs: BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>> =
        params_specs_full.iter().map(|(r, s)| (r.clone(), s.fields.clone())).collect();

    let (model_lcs, model_registry) = crate::lower::lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        view_extras,
        &params_specs,
    );
    library::register_class_hierarchy(&model_lcs);
    for lc in &model_lcs {
        files.push(library::emit_class_file(lc));
    }

    // Shared lowering extras: the model registry + the (preliminary) view
    // class info, so controller/jbuilder bodies dispatch model attributes and
    // view-module calls.
    let mut base_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    base_extras.extend(crate::lower::extras_from_lcs(&preliminary_views));

    // Route helpers (`RouteHelpers.article_path(id)`) → a `static class`. The
    // controllers' redirects/links dispatch against these.
    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    if let Some(f) = library::emit_function_module(&route_helper_funcs) {
        files.push(f);
    }

    // Jbuilder (`.json.jbuilder`) view render methods — lowered only to harvest
    // their method names (`indexJson`, `showJson`) for the view stubs the
    // controllers' JSON branches call.
    let jbuilder_lcs =
        crate::lower::lower_jbuilder_to_library_classes(&app.views, app, base_extras.clone());

    // Controllers. The synthesized `<Resource>Params` siblings are origin-
    // tagged and route to `app/models` alongside the models; the real
    // controllers route to `app/controllers`.
    let mut controller_extras = base_extras;
    controller_extras.extend(crate::lower::extras_from_funcs(&route_helper_funcs));
    let assocs = crate::lower::model_associations::compute_association_graph(app);
    let controller_lcs = crate::lower::lower_controllers_with_arel_views_and_assocs(
        &app.controllers,
        controller_extras,
        Some(&app.schema),
        &app.views,
        &assocs,
    );

    // Synthesize `ApplicationController` when a controller extends it but the
    // app doesn't define one (Rails scaffolds assume it).
    let needs_app_controller = app
        .controllers
        .iter()
        .any(|c| matches!(c.parent.as_ref(), Some(p) if p.0.as_str() == "ApplicationController"))
        && !app.controllers.iter().any(|c| c.name.0.as_str() == "ApplicationController");
    if needs_app_controller {
        files.push(EmittedFile {
            path: std::path::PathBuf::from("app/controllers/ApplicationController.cs"),
            content: "namespace Roundhouse;\n\npublic class ApplicationController : ActionControllerBase\n{\n}\n".to_string(),
        });
    }

    // Register the view-module methods so a controller's `Articles.new(...)` /
    // `Articles.index(...)` resolves as a *method call*, not a constructor
    // (the views themselves are stubbed until Phase 4).
    for lc in preliminary_views.iter().chain(jbuilder_lcs.iter()) {
        let module = naming::type_name(lc.name.0.as_str());
        for m in &lc.methods {
            let params = m.params.iter().map(|p| naming::camel(p.name.as_str())).collect();
            expr::register_method_params(&module, m.name.as_str(), params);
        }
    }

    library::register_class_hierarchy(&controller_lcs);
    for lc in &controller_lcs {
        let dir = if lc.origin.is_some() { "app/models" } else { "app/controllers" };
        files.push(library::emit_class_file_in(lc, dir));
    }

    // View stubs (html + jbuilder method names): the broadcast callbacks and
    // controller render branches reference view modules whose real
    // string-builder renderers land in Phase 4. Stub each so the app compiles.
    if let Some(stub) = emit_view_stubs(&preliminary_views, &jbuilder_lcs) {
        files.push(stub);
    }

    // Program.cs — the entry point wiring the routes table + controller factory
    // map + layout into the Kestrel Server.
    files.push(emit_program(app));

    files
}

/// `Program.cs` — top-level statements building the routes table + controller
/// factory map (app-specific) and handing them to `Server.Start`.
fn emit_program(app: &App) -> EmittedFile {
    let (route_lines, ctrl_lines) = route_table_literals(app);
    // The layout wraps every html response; `Layouts.application` when the app
    // has a layout (identity otherwise).
    let has_layout = app.views.iter().any(|v| v.name.as_str() == "layouts/application");
    let layout = if has_layout {
        "(body, notice, alert) => Layouts.application(body, notice, alert)"
    } else {
        "(body, notice, alert) => body"
    };
    let content = format!(
        "// Generated by Roundhouse (csharp). Entry point — wires the routes\n\
         // table + controllers into the Kestrel Server primitive.\n\n\
         using Roundhouse;\n\
         // Disambiguate from Microsoft.AspNetCore.Routing.Route (web SDK implicit using).\n\
         using Route = Roundhouse.Route;\n\n\
         var port = int.Parse(Environment.GetEnvironmentVariable(\"PORT\") ?? \"3000\");\n\n\
         var routes = new List<Route>\n{{\n{}\n}};\n\n\
         var controllers = new Dictionary<string, Func<ActionControllerBase>>\n{{\n{}\n}};\n\n\
         Func<string, string?, string?, string> layout = {layout};\n\n\
         Server.Start(port, routes, controllers, layout);\n",
        route_lines.join("\n"),
        ctrl_lines.join("\n"),
    );
    EmittedFile { path: std::path::PathBuf::from("Program.cs"), content }
}

/// The routes-table + controller-factory-map C# literal lines for `Program.cs`.
fn route_table_literals(app: &App) -> (Vec<String>, Vec<String>) {
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
                "    new Route({:?}, {:?}, {:?}, {:?}),",
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
        controllers.iter().map(|c| format!("    [{c:?}] = () => new {c}(),")).collect();
    (route_lines, ctrl_lines)
}

/// Emit `app/views/ViewStubs.cs` — one `static class` per view module, with a
/// `params object?[]`-accepting stub per method (html + jbuilder/JSON),
/// returning `""`. The model broadcast callbacks + the controllers' render
/// branches call these; Phase 4 replaces the stubs with the real
/// string-builder view renderers.
fn emit_view_stubs(
    preliminary_views: &[crate::dialect::LibraryClass],
    jbuilder_lcs: &[crate::dialect::LibraryClass],
) -> Option<EmittedFile> {
    let mut by_module: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for lc in preliminary_views.iter().chain(jbuilder_lcs.iter()) {
        let module = naming::type_name(lc.name.0.as_str());
        let methods = by_module.entry(module).or_default();
        for m in &lc.methods {
            methods.insert(naming::camel(m.name.as_str()));
        }
    }
    if by_module.is_empty() {
        return None;
    }
    let mut content = String::from(
        "// Generated by Roundhouse (csharp). Phase-2 view stubs — the model\n\
         // broadcast callbacks reference these; Phase 4 emits the real views.\n\n\
         namespace Roundhouse;\n\n",
    );
    for (module, methods) in by_module {
        content.push_str(&format!("public static class {module} {{\n"));
        for m in methods {
            content.push_str(&format!(
                "    public static string {m}(params object?[] args) => \"\";\n"
            ));
        }
        content.push_str("}\n\n");
    }
    Some(EmittedFile {
        path: std::path::PathBuf::from("app/views/ViewStubs.cs"),
        content,
    })
}
