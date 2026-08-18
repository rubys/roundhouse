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
    let vctx = crate::lower::ViewLowerCtx::new(app);
    let preliminary_views: Vec<crate::dialect::LibraryClass> = app
        .views
        .iter()
        .map(|v| vctx.lower(v))
        .collect();
    let view_extras = crate::lower::extras_from_lcs(&preliminary_views);

    // Permitted-params specs → each model gains a typed `from_params`.
    let params_specs =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);

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

    // Shared lowering extras for views/jbuilder/controllers: the model
    // registry + (preliminary) view class info + route-helper + importmap
    // function signatures.
    let mut view_lower_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    view_lower_extras.extend(crate::lower::extras_from_lcs(&preliminary_views));

    // Route helpers (`RouteHelpers.article_path(id)`) → a `static class`.
    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    view_lower_extras.extend(crate::lower::extras_from_funcs(&route_helper_funcs));
    if let Some(f) = library::emit_function_module(&route_helper_funcs) {
        files.push(f);
    }

    // Importmap (`javascript_importmap_tags`) helpers → a `static class`.
    let importmap_funcs = crate::lower::lower_importmap_to_library_functions(app);
    view_lower_extras.extend(crate::lower::extras_from_funcs(&importmap_funcs));
    if let Some(f) = library::emit_function_module(&importmap_funcs) {
        files.push(f);
    }

    // Views: each ERB template lowers to a string-builder render method on its
    // `Views::<Plural>` module; jbuilder templates lower to `<name>_json`
    // methods on the same module. C# static classes can't be reopened, so the
    // per-template LibraryClasses merge into one `static class <Plural>` per
    // module, emitted to app/views/.
    let view_lcs =
        crate::lower::lower_views_to_library_classes(&app.views, app, view_lower_extras.clone());
    let jbuilder_lcs =
        crate::lower::lower_jbuilder_to_library_classes(&app.views, app, view_lower_extras.clone());
    let mut all_view_lcs = view_lcs.clone();
    all_view_lcs.extend(jbuilder_lcs);
    let merged_views = merge_by_module(all_view_lcs);
    library::register_class_hierarchy(&merged_views);
    // Register the view-module methods so a controller's `Articles.new(...)` /
    // `Articles.index(...)` resolves as a method call, not a constructor.
    for lc in &merged_views {
        let module = naming::type_name(lc.name.0.as_str());
        for m in &lc.methods {
            let params = m.params.iter().map(|p| naming::camel(p.name.as_str())).collect();
            expr::register_method_params(&module, m.name.as_str(), params);
        }
    }
    for lc in &merged_views {
        files.push(library::emit_class_file_in(lc, "app/views"));
    }

    // Controllers. The synthesized `<Resource>Params` siblings are origin-
    // tagged and route to `app/models`; the real controllers to
    // `app/controllers`.
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

    library::register_class_hierarchy(&controller_lcs);
    for lc in &controller_lcs {
        let dir = if lc.origin.is_some() { "app/models" } else { "app/controllers" };
        files.push(library::emit_class_file_in(lc, dir));
    }

    // Program.cs — the entry point wiring the routes table + controller factory
    // map + layout into the Kestrel Server.
    files.push(emit_program(app));

    // Test project → `tests/` — one xUnit class per ingested TestModule, the
    // `<Plural>Fixtures` loaders, the generated wiring (TestSetup.cs), the
    // hand-written base (TestSupport.cs) and its own csproj. Emitted only
    // when the App carries tests; a production build has no `tests/` dir at
    // all, and the app csproj excludes it either way (see package::CSPROJ).
    if !app.test_modules.is_empty() {
        files.extend(emit_tests(app, &model_lcs, &view_lcs, &controller_lcs));
    }

    files
}

/// The `tests/` project: fixtures, the lowered test classes, the generated
/// `TestSetup.cs` wiring, `TestSupport.cs`, and `App.Tests.csproj`.
///
/// Sibling of the kotlin/swift test legs. The body-typer needs the framework
/// RBS + app/runtime ClassInfo to dispatch precisely against framework
/// methods (`Article.find`, `RouteHelpers.article_path`, …); without it
/// `Ty::Untyped → object?` collapse loses the typed dispatch.
fn emit_tests(
    app: &App,
    model_lcs: &[crate::dialect::LibraryClass],
    view_lcs: &[crate::dialect::LibraryClass],
    controller_lcs: &[crate::dialect::LibraryClass],
) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    let mut test_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> = Vec::new();
    for (class_id, methods) in &app.rbs_signatures {
        let mut info = crate::analyze::ClassInfo::default();
        for (m_name, m_ty) in methods {
            info.instance_methods.insert(m_name.clone(), m_ty.clone());
        }
        test_extras.push((class_id.clone(), info));
    }
    test_extras.extend(crate::lower::extras_from_lcs(model_lcs));
    test_extras.extend(crate::lower::extras_from_lcs(view_lcs));
    test_extras.extend(crate::lower::extras_from_lcs(controller_lcs));

    // `<Plural>Fixtures` classes — one per fixture YAML. Each exposes
    // per-label class methods (`ArticlesFixtures.One()` → `Article.Find(1)`)
    // plus `_fixtures_load!`, which RoundhouseTestCase invokes after each
    // schema reset. Registered so test bodies type fixture reads
    // (`@article = articles(:one)` infers Article).
    let fixture_lcs = crate::lower::lower_fixtures_to_library_classes(app);
    test_extras.extend(crate::lower::extras_from_lcs(&fixture_lcs));
    library::register_class_hierarchy(&fixture_lcs);
    for lc in &fixture_lcs {
        files.push(library::emit_class_file_in(lc, "tests"));
    }

    let test_lowered = crate::lower::lower_test_modules_with_inner(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        test_extras,
        &crate::lower::routes::helper_id_segments(app),
    );
    for lowered in &test_lowered {
        files.push(library::emit_test_class_file(
            &lowered.test_class,
            &lowered.inner_classes,
            &lowered.constants,
        ));
    }

    files.push(emit_test_setup(app, &fixture_lcs));
    files.push(EmittedFile {
        path: std::path::PathBuf::from("tests/TestSupport.cs"),
        content: TEST_SUPPORT_CS.to_string(),
    });
    files.extend(package::test_scaffold());
    files
}

/// The hand-written half of the test harness (the app-specific half is the
/// generated TestSetup.cs): `RoundhouseTestCase` — the base every emitted
/// test class extends — and the `Dom` stub `AssertSelect` queries through.
const TEST_SUPPORT_CS: &str = include_str!("../../runtime/csharp/TestSupport.cs");

/// `tests/TestSetup.cs` — the app-specific test wiring `RoundhouseTestCase`
/// consumes by fixed name: the schema DDL (replayed before every test), the
/// fixture loaders, and the routes/controllers tables for the controller-test
/// dispatch (the same builders Program.cs uses).
fn emit_test_setup(app: &App, fixture_lcs: &[crate::dialect::LibraryClass]) -> EmittedFile {
    let schema_sql =
        crate::emit::shared::schema_sql::render_schema_statements(&app.schema).join(";\n");
    let schema_lit = schema_sql.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");

    let loader_lines: Vec<String> = fixture_lcs
        .iter()
        .map(|lc| {
            let name = lc.name.0.as_str().rsplit("::").next().unwrap_or(lc.name.0.as_str());
            // `pascal` preserves leading underscores, so `_fixtures_load!`
            // emits as `_FixturesLoadBang`.
            format!("        () => {name}._FixturesLoadBang(),")
        })
        .collect();

    let (route_lines, ctrl_lines) = route_table_literals(app);

    let content = format!(
        "// Generated by Roundhouse (csharp). App-specific test wiring —\n\
         // consumed by RoundhouseTestCase (TestSupport.cs).\n\n\
         using System;\n\
         using System.Collections.Generic;\n\n\
         namespace Roundhouse;\n\n\
         public static class RoundhouseTestSetup\n{{\n\
         \x20\x20\x20\x20public const string SchemaSql = \"{schema_lit}\";\n\n\
         \x20\x20\x20\x20public static readonly List<Action> FixtureLoaders = new()\n    {{\n{}\n    }};\n\n\
         \x20\x20\x20\x20public static readonly List<Route> Routes = new()\n    {{\n{}\n    }};\n\n\
         \x20\x20\x20\x20public static readonly Dictionary<string, Func<ActionControllerBase>> Controllers = new()\n    {{\n{}\n    }};\n\
         }}\n",
        loader_lines.join("\n"),
        route_lines.join("\n"),
        ctrl_lines.join("\n"),
    );
    EmittedFile { path: std::path::PathBuf::from("tests/TestSetup.cs"), content }
}

/// `Program.cs` — top-level statements building the routes table + controller
/// factory map (app-specific) and handing them to `Server.Start`.
fn emit_program(app: &App) -> EmittedFile {
    let (route_lines, ctrl_lines) = route_table_literals(app);
    // The layout wraps every html response; `Layouts.application` when the app
    // has a layout (identity otherwise).
    let has_layout = app.views.iter().any(|v| v.name.as_str() == "layouts/application");
    let layout = if has_layout {
        "(body, notice, alert) => Layouts.Application(body, notice, alert)"
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

/// Merge `LibraryClass`es that share a module name into one (concatenating
/// their methods), preserving first-seen order. The view lowerer produces one
/// LC per template, several sharing a `Views::<Plural>` name; C# `static
/// class`es can't be reopened across declarations, so they collapse into a
/// single class before emit.
fn merge_by_module(
    lcs: Vec<crate::dialect::LibraryClass>,
) -> Vec<crate::dialect::LibraryClass> {
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
