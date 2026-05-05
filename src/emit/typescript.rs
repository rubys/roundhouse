//! TypeScript emitter — kind-agnostic LibraryClass walker.
//!
//! Phase B of the rewrite (2026-04-30): the emitter no longer knows
//! about views, controllers, models, schema, routes, or fixtures as
//! distinct output kinds. Every input flows through the lowerer
//! pipeline into `LibraryClass` and is rendered by
//! `library::emit_class_file`. Per-target surface = `expr.rs` (Expr →
//! TS syntax) + `ty.rs` (Ty → TS type) + `library.rs` (LibraryClass
//! walker) + ecosystem files (`package.json`, `tsconfig.json`,
//! `juntos.ts` runtime stub).
//!
//! Outputs not yet covered: controllers, schema, routes, importmap,
//! fixtures, specs. Each is a missing `*_to_library` lowerer (see
//! `project_universal_post_lowering_ir`); when the lowerer lands the
//! output joins the walker without changes here.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::ty::Ty;

const JUNTOS_STUB_SOURCE: &str = include_str!("../../runtime/typescript/juntos.ts");
const MINITEST_RUNTIME_SOURCE: &str = include_str!("../../runtime/typescript/minitest.ts");

/// Framework runtime files inlined at the canonical `src/<name>.ts`
/// path. Internal cross-imports use `./<name>.js` so emitting all of
/// them under the same flat directory satisfies module resolution.
/// Excludes `juntos.ts` (mapped via tsconfig path alias) and
/// `minitest.ts` (lives under `test/_runtime/`); both are handled
/// separately by their own emit slots.
/// Hand-written TS runtime files, baked in via `include_str!` and
/// shipped as-is. These are target-specific primitives (DB adapter
/// shim, HTTP server glue, test runner adapter) that don't have a
/// Ruby source — they're authored directly for TS.
///
/// The transpile-from-Ruby files (active_record_base, validations,
/// errors, parameters, router, action_controller_base, inflector)
/// are no longer here — they're produced inline at emit time by
/// `crate::runtime_loader::typescript_units(...)` (option (c)).
const RUNTIME_FILES: &[(&str, &str)] = &[
    (
        "src/broadcasts.ts",
        include_str!("../../runtime/typescript/broadcasts.ts"),
    ),
    ("src/server.ts", include_str!("../../runtime/typescript/server.ts")),
];

mod expr;
mod library;
mod naming;
mod package;
mod ty;

pub use ty::{ts_return_ty, ts_ty};

/// Public re-export so `runtime_loader` can render module-level
/// constant values (`HTML_ESCAPES = { ... }.freeze` from
/// `view_helpers.rb`) as top-level `const NAME = ...;` declarations
/// in the transpiled output. The constant body uses the same
/// expression emitter every method body does.
pub fn emit_expr_for_runtime(e: &crate::expr::Expr) -> String {
    expr::emit_expr(e)
}

/// Emit a TypeScript project for `app`. Every artifact (models,
/// views, controllers, fixtures, tests, schema) flows through the
/// universal walker. A single shared class registry is threaded
/// through all lowerings so cross-class dispatch (`Article.find(...)`
/// from a controller body, `ArticlesFixtures.one()` from a test)
/// types end-to-end.
pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    files.push(package::emit_package_json());
    files.push(package::emit_tsconfig_json(app));
    files.push(EmittedFile {
        path: PathBuf::from("src/juntos.ts"),
        content: JUNTOS_STUB_SOURCE.to_string(),
    });

    // Framework runtime files: hand-written TS primitives (HTTP
    // server, DB adapter shim, test runner glue) are inlined as-is
    // from `runtime/typescript/`. Layout: flat under `src/`;
    // internal cross-imports use `./<name>.js`.
    for (path, content) in RUNTIME_FILES {
        files.push(EmittedFile {
            path: PathBuf::from(*path),
            content: (*content).to_string(),
        });
    }

    // (Transpile-from-Ruby runtime emit deferred to AFTER the
    // lowering pipeline so the tree-shake walker can use the
    // lowered app classes as reachability roots.)

    // ── Lowering pipeline ───────────────────────────────────────────
    // Order matters because each step's output feeds the next's
    // shared registry. Views are lowered twice — once preliminarily
    // (without model knowledge) so models can dispatch on Views::*,
    // then again with the full model registry so view bodies can
    // dispatch on models.

    let preliminary_views: Vec<crate::dialect::LibraryClass> = app
        .views
        .iter()
        .map(|v| crate::lower::lower_view_to_library_class(v, app))
        .collect();
    let view_extras = library::extras_from_lcs(&preliminary_views);

    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    let route_helper_extras = library::extras_from_funcs(&route_helper_funcs);

    // Collect controller `permit(...)` declarations once so the model
    // lowerer can synthesize `from_params(p: <Resource>Params)` factories
    // matching the permitted-fields list. The same specs feed the
    // controller lowerer below — both call sites need the same view
    // of the controller-derived metadata.
    let params_specs_full =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    let params_specs_simple: std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>> =
        params_specs_full
            .iter()
            .map(|(r, s)| (r.clone(), s.fields.clone()))
            .collect();
    let (model_lcs, model_registry) = crate::lower::lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        view_extras,
        &params_specs_simple,
    );

    let mut view_lower_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.clone().into_iter().collect();
    view_lower_extras.extend(route_helper_extras.clone());
    let view_lcs = crate::lower::lower_views_to_library_classes(
        &app.views,
        app,
        view_lower_extras,
    );

    let mut controller_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    controller_extras.extend(library::extras_from_lcs(&view_lcs));
    controller_extras.extend(route_helper_extras);
    let controller_lcs = crate::lower::lower_controllers_to_library_classes(
        &app.controllers,
        controller_extras.clone(),
    );

    let fixture_lcs = crate::lower::lower_fixtures_to_library_classes(app);

    let test_lowered: Vec<crate::lower::LoweredTestModule> = if app.test_modules.is_empty() {
        Vec::new()
    } else {
        let mut test_extras = controller_extras;
        test_extras.extend(library::extras_from_lcs(&controller_lcs));
        test_extras.extend(library::extras_from_lcs(&fixture_lcs));
        crate::lower::lower_test_modules_with_inner(
            &app.test_modules,
            &app.fixtures,
            &app.models,
            test_extras,
        )
    };
    let test_lcs: Vec<crate::dialect::LibraryClass> = test_lowered
        .iter()
        .map(|m| m.test_class.clone())
        .collect();
    // Inner classes are file-scoped to their owning test class; they
    // need to be reachable for treeshake (so methods on Validatable
    // aren't dropped) but get emitted INSIDE each test file rather
    // than as their own files.
    let test_inner_lcs: Vec<crate::dialect::LibraryClass> = test_lowered
        .iter()
        .flat_map(|m| m.inner_classes.iter().cloned())
        .collect();

    // ── Emit ────────────────────────────────────────────────────────

    // Tree-shake the framework runtime: walk all app-side method
    // bodies (models, controllers, views, fixtures, tests) and
    // filter the runtime LibraryClasses to only the methods reached
    // transitively. `validates_format_of` etc. that the app never
    // uses get dropped from `validations.ts`. Conservative on
    // untyped Sends — keeps the method on every class that defines
    // it, never wrong.
    let mut all_app_classes: Vec<crate::dialect::LibraryClass> =
        Vec::with_capacity(model_lcs.len() + view_lcs.len() + controller_lcs.len() + fixture_lcs.len() + test_lcs.len() + test_inner_lcs.len());
    all_app_classes.extend(model_lcs.iter().cloned());
    all_app_classes.extend(view_lcs.iter().cloned());
    all_app_classes.extend(controller_lcs.iter().cloned());
    all_app_classes.extend(fixture_lcs.iter().cloned());
    all_app_classes.extend(test_lcs.iter().cloned());
    all_app_classes.extend(test_inner_lcs.iter().cloned());

    // First pass: parse-only to get the runtime classes for
    // reachability. `typescript_units` parses + emits in one step;
    // we need to seed the reachability walker with the runtime's
    // own classes so cross-references between (e.g.) Base and
    // Validations resolve. The parse happens twice — once here for
    // reachability, once below for the actual emit. Cheap (~10ms
    // for the framework runtime).
    let runtime_units_seed = crate::runtime_loader::typescript_units(|_, c| c)
        .expect("runtime transpile parse failed");
    // Build the runtime alias list: each LibraryClass appears under
    // its simple name AND its qualified name (`ActiveRecord::Base`).
    // Two `Base` classes (one from AR, one from ActionController)
    // would collide on the simple name; the qualified alias
    // disambiguates parent-chain lookups from app-side classes.
    let runtime_aliases: Vec<(crate::ident::ClassId, &crate::dialect::LibraryClass)> =
        runtime_units_seed
            .iter()
            .flat_map(|u| {
                u.classes.iter().flat_map(move |c| {
                    let mut entries = vec![(c.name.clone(), c)];
                    if !u.namespace.is_empty() {
                        let qualified = crate::ident::ClassId(crate::ident::Symbol::from(
                            format!("{}::{}", u.namespace, c.name.0.as_str()),
                        ));
                        entries.push((qualified, c));
                    }
                    entries
                })
            })
            .collect();
    // Hand-written runtime files (server.ts, test_support.ts,
    // broadcasts.ts) call into transpiled framework methods that the
    // app-body walk wouldn't otherwise see. Each RuntimeEntry can
    // declare its `(class, method)` pairs so treeshake keeps them.
    let extra_roots: Vec<(crate::ident::ClassId, crate::ident::Symbol)> = runtime_units_seed
        .iter()
        .flat_map(|u| {
            u.extra_roots
                .iter()
                .map(|(cls, method)| {
                    (
                        crate::ident::ClassId(crate::ident::Symbol::from(*cls)),
                        crate::ident::Symbol::from(*method),
                    )
                })
        })
        .collect();
    // App-side standalone functions (seeds, route helpers, schema,
    // importmap, routes dispatch) carry app code too. Their bodies
    // are roots — `Article.create!(...)` in seeds.rb needs to keep
    // `create!` alive on Base.
    let mut all_app_functions: Vec<crate::dialect::LibraryFunction> = Vec::new();
    all_app_functions.extend(crate::lower::lower_seeds_to_library_functions(app));
    all_app_functions.extend(route_helper_funcs.clone());
    let reach = crate::treeshake::Reachability::from_app_roots(
        &all_app_classes,
        &runtime_aliases,
        &all_app_functions,
        &extra_roots,
    );

    let runtime_units = crate::runtime_loader::typescript_units(|_path, classes| {
        classes
            .into_iter()
            .map(|c| crate::treeshake::filter_runtime_class(&c, &reach))
            .collect()
    })
    .expect("runtime transpile failed (Ruby source error)");
    for unit in runtime_units {
        files.push(EmittedFile {
            path: unit.out_path,
            content: unit.content,
        });
    }

    let schema_funcs = crate::lower::lower_schema_to_library_functions(&app.schema);
    if !schema_funcs.is_empty() {
        files.push(library::emit_module_file(
            &schema_funcs,
            app,
            PathBuf::from("src/schema.ts"),
        ));
    }

    if !route_helper_funcs.is_empty() {
        files.push(library::emit_module_file(
            &route_helper_funcs,
            app,
            PathBuf::from("app/route_helpers.ts"),
        ));
    }

    let routes_dispatch_funcs = crate::lower::lower_routes_to_dispatch_functions(app);
    if !routes_dispatch_funcs.is_empty() {
        files.push(library::emit_module_file(
            &routes_dispatch_funcs,
            app,
            PathBuf::from("app/routes.ts"),
        ));
    }

    let importmap_funcs = crate::lower::lower_importmap_to_library_functions(app);
    if !importmap_funcs.is_empty() {
        files.push(library::emit_module_file(
            &importmap_funcs,
            app,
            PathBuf::from("app/importmap.ts"),
        ));
    }

    let has_seeds = app.seeds.is_some();
    let seeds_funcs = crate::lower::lower_seeds_to_library_functions(app);
    if !seeds_funcs.is_empty() {
        files.push(library::emit_module_file(
            &seeds_funcs,
            app,
            PathBuf::from("db/seeds.ts"),
        ));
    }

    // Synthesized siblings (`<Model>Row` from models, `<Resource>Params`
    // from controllers) carry an `origin` tag. Combine both into one
    // list so render_imports recognizes them as model-style imports —
    // they all live in `app/models/` regardless of which lowerer
    // produced them.
    let mut synthesized_names: Vec<String> = model_lcs
        .iter()
        .chain(controller_lcs.iter())
        .filter(|lc| lc.origin.is_some())
        .map(|lc| lc.name.0.as_str().to_string())
        .collect();
    synthesized_names.sort();
    synthesized_names.dedup();
    for lc in &model_lcs {
        let stem = crate::naming::snake_case(lc.name.0.as_str());
        let out_path = PathBuf::from(format!("app/models/{stem}.ts"));
        files.push(library::emit_class_file_with_synthesized(
            lc,
            app,
            out_path,
            &synthesized_names,
        ));
    }

    // Views: flatten the per-template LibraryClasses into
    // LibraryFunctions and emit one function per file. The body-typer
    // registry above (`view_extras` / `extras_from_lcs(&view_lcs)`)
    // still uses the class shape so cross-class dispatch
    // (`Views::Articles.article(x)`) types correctly without a
    // parallel registry. The class-vs-function choice is purely an
    // emit-side surface decision.
    let view_funcs = crate::lower::flatten_lcs_to_functions(&view_lcs);
    for (view, func) in app.views.iter().zip(view_funcs.iter()) {
        let out_path = view_output_path(view.name.as_str());
        files.push(library::emit_function_file(func, app, out_path));
    }
    if !view_funcs.is_empty() {
        files.push(library::emit_views_aggregator(&app.views, &view_funcs));
    }

    // Synthesized `<Resource>Params` classes ride in `controller_lcs`
    // (origin tagged); route those to `app/models/` rather than
    // `app/controllers/`. Use the combined `synthesized_names` so a
    // controller body's reference to a Row class (or any other
    // synthesized class) resolves uniformly.
    for lc in &controller_lcs {
        let stem = crate::naming::snake_case(lc.name.0.as_str());
        let out_path = if lc.origin.is_some() {
            PathBuf::from(format!("app/models/{stem}.ts"))
        } else {
            PathBuf::from(format!("app/controllers/{stem}.ts"))
        };
        files.push(library::emit_class_file_with_synthesized(
            lc,
            app,
            out_path,
            &synthesized_names,
        ));
    }

    for lc in &app.library_classes {
        let stem = crate::naming::snake_case(lc.name.0.as_str());
        let out_path = PathBuf::from(format!("app/models/{stem}.ts"));
        files.push(library::emit_class_file(lc, app, out_path));
    }

    for lc in &fixture_lcs {
        let stem = fixture_file_stem(lc.name.0.as_str());
        let out_path = PathBuf::from(format!("test/fixtures/{stem}.ts"));
        files.push(library::emit_class_file(lc, app, out_path));
    }

    if !test_lcs.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("test/_runtime/minitest.ts"),
            content: MINITEST_RUNTIME_SOURCE.to_string(),
        });
        files.push(emit_test_setup_ts(app, &fixture_lcs));
        for lowered in &test_lowered {
            let lc = &lowered.test_class;
            let stem = test_file_stem(lc.name.0.as_str());
            let out_path = PathBuf::from(format!("test/{stem}.test.ts"));
            // Inner classes (declared inline inside the test class
            // body in Ruby) hoist to file scope as companions; share
            // the test file's import header so framework dependencies
            // (`Validations`, `ActiveRecordBase`, etc.) resolve.
            let mut emitted = library::emit_class_file_with_companions(
                lc,
                app,
                out_path.clone(),
                &[],
                &lowered.inner_classes,
            );

            emitted.content.push('\n');
            // setup.ts runs schema + adapter + routes + fixtures setup
            // at module-load time. node:test loads each `.test.ts`
            // file independently in the same process; the first
            // file-load triggers setup, subsequent loads no-op.
            // Imported BEFORE discover_tests so registration sees
            // the prepared world.
            emitted.content.push_str(&format!(
                "import \"./_runtime/setup.js\";\n\
                 import {{ discover_tests }} from \"./_runtime/minitest.js\";\n\
                 discover_tests({});\n",
                lc.name.0.as_str(),
            ));
            files.push(emitted);
        }
    }

    files.push(emit_main_ts(app, has_seeds));

    files
}

/// Hand-written `main.ts` shell. Wires together the generated
/// schema, optional seeds, the routes dispatch table, the imported
/// controller classes, and the runtime's `startServer`. The server
/// owns request → controller dispatch (via `Router.match` and the
/// `controllers` map keyed by controller-symbol).
fn emit_main_ts(app: &App, has_seeds: bool) -> EmittedFile {
    let mut s = String::new();
    s.push_str("// Generated by Roundhouse.\n");
    s.push_str("import { startServer } from \"./src/server.js\";\n");
    if !app.schema.tables.is_empty() {
        s.push_str("import { Schema } from \"./src/schema.js\";\n");
    }
    if has_seeds {
        s.push_str("import { Seeds } from \"./db/seeds.js\";\n");
    }
    let flat = crate::lower::flatten_routes(app);
    let has_routes = !flat.is_empty();
    // Mirror the routes lowerer's "root iff path == \"/\"" partition.
    let has_root = flat.iter().any(|r| r.path == "/");
    if has_routes {
        s.push_str("import { Routes } from \"./app/routes.js\";\n");
    }
    // The application layout transpiles to a `(body) => string` function
    // exported as `application` from `app/views/layouts/application.ts`.
    // Wiring it into `startServer({ layout })` makes the dispatcher wrap
    // each response in the real Rails-shaped <head>/<body> instead of
    // server.ts's fallback shell.
    let has_layout = app
        .views
        .iter()
        .any(|v| v.name.as_str() == "layouts/application");
    if has_layout {
        s.push_str(
            "import { application as renderLayoutsApplication } \
             from \"./app/views/layouts/application.js\";\n",
        );
    }
    for c in &app.controllers {
        let stem = crate::naming::snake_case(c.name.0.as_str());
        let class_name = c.name.0.as_str();
        s.push_str(&format!(
            "import {{ {class_name} }} from \"./app/controllers/{stem}.js\";\n"
        ));
    }
    s.push('\n');
    s.push_str("await startServer({\n");
    if app.schema.tables.is_empty() {
        s.push_str("  schemaStatements: [],\n");
    } else {
        s.push_str("  schemaStatements: Schema.statements(),\n");
    }
    if has_seeds {
        s.push_str("  seeds: () => Seeds.run(),\n");
    }
    if has_layout {
        s.push_str("  layout: renderLayoutsApplication,\n");
    }
    if has_routes {
        s.push_str("  routes: Routes.table(),\n");
        if has_root {
            s.push_str("  rootRoute: Routes.root(),\n");
        }
        s.push_str("  controllers: {\n");
        for c in &app.controllers {
            let class_name = c.name.0.as_str();
            // `ArticlesController` → `articles` — same convention as
            // `controller_symbol` in the routes lowerer.
            let symbol = crate::naming::snake_case(
                class_name.strip_suffix("Controller").unwrap_or(class_name),
            );
            s.push_str(&format!("    {symbol}: {class_name},\n"));
        }
        s.push_str("  },\n");
    } else {
        s.push_str("  routes: [],\n");
        s.push_str("  controllers: {},\n");
    }
    s.push_str("});\n");
    EmittedFile {
        path: PathBuf::from("main.ts"),
        content: s,
    }
}

/// Map a view name (`articles/index`, `articles/_article`,
/// `layouts/application`) to the output path under `app/views/`.
fn view_output_path(view_name: &str) -> PathBuf {
    PathBuf::from(format!("app/views/{view_name}.ts"))
}

/// `ArticleTest` → `article` (strip Test suffix, snake_case). Used
/// for the `test/<stem>.test.ts` output path so the file name reads
/// naturally without redundant `_test_test`.
fn test_file_stem(class_name: &str) -> String {
    let stem = class_name.strip_suffix("Test").unwrap_or(class_name);
    crate::naming::snake_case(stem)
}

/// `ArticlesFixtures` → `articles` (strip Fixtures suffix, snake_case).
fn fixture_file_stem(class_name: &str) -> String {
    let stem = class_name.strip_suffix("Fixtures").unwrap_or(class_name);
    crate::naming::snake_case(stem)
}

/// Emit `test/_runtime/setup.ts` — runs once on first import (each
/// `.test.ts` file imports it before calling `discover_tests`). Opens
/// an in-memory SQLite DB, applies the schema, installs the adapter
/// (via `installDb`'s wire-up), installs routes + controllers for
/// `this.get/post/...` dispatch, and loads every fixture class's
/// `_fixtures_load_bang()`. Mirrors the spinel-side `SchemaSetup`
/// + `FixtureLoader` setup in `runtime/spinel/test/test_helper.rb`.
fn emit_test_setup_ts(
    app: &App,
    fixture_lcs: &[crate::dialect::LibraryClass],
) -> EmittedFile {
    let mut s = String::new();
    s.push_str("// Generated by Roundhouse.\n");
    s.push_str("import Database from \"better-sqlite3\";\n\n");
    s.push_str("import { installDb } from \"../../src/juntos.js\";\n");

    let has_schema = !app.schema.tables.is_empty();
    if has_schema {
        s.push_str("import { Schema } from \"../../src/schema.js\";\n");
    }

    let flat = crate::lower::flatten_routes(app);
    let has_routes = !flat.is_empty();
    let has_root = flat.iter().any(|r| r.path == "/");
    if has_routes {
        s.push_str("import { installRoutes } from \"./minitest.js\";\n");
        s.push_str("import { Routes } from \"../../app/routes.js\";\n");
        for c in &app.controllers {
            let stem = crate::naming::snake_case(c.name.0.as_str());
            let class_name = c.name.0.as_str();
            s.push_str(&format!(
                "import {{ {class_name} }} from \"../../app/controllers/{stem}.js\";\n",
            ));
        }
    }

    for lc in fixture_lcs {
        let stem = fixture_file_stem(lc.name.0.as_str());
        let class_name = lc.name.0.as_str();
        s.push_str(&format!(
            "import {{ {class_name} }} from \"../fixtures/{stem}.js\";\n",
        ));
    }

    s.push('\n');
    s.push_str("const db = new Database(\":memory:\");\n");
    if has_schema {
        s.push_str("for (const stmt of Schema.statements()) {\n");
        s.push_str("  db.exec(stmt);\n");
        s.push_str("}\n");
    }
    // installDb wires the SqliteActiveRecordAdapter onto
    // ActiveRecord.adapter (juntos.ts) so framework Ruby's
    // `ActiveRecord.adapter.find/all/...` resolves.
    s.push_str("installDb(db);\n");

    if has_routes {
        s.push_str("installRoutes(\n");
        s.push_str("  Routes.table(),\n");
        if has_root {
            s.push_str("  Routes.root(),\n");
        } else {
            s.push_str("  undefined,\n");
        }
        s.push_str("  {\n");
        for c in &app.controllers {
            let class_name = c.name.0.as_str();
            let symbol = crate::naming::snake_case(
                class_name.strip_suffix("Controller").unwrap_or(class_name),
            );
            s.push_str(&format!("    {symbol}: {class_name},\n"));
        }
        s.push_str("  },\n");
        s.push_str(");\n");
    }

    if !fixture_lcs.is_empty() {
        s.push('\n');
        s.push_str("// Load every fixture class's data into the in-memory DB.\n");
        s.push_str("// Loaded once at module-init; tests share state across\n");
        s.push_str("// runs in this suite. Per-test isolation (transaction\n");
        s.push_str("// rollback) is a future improvement.\n");
        for lc in fixture_lcs {
            let class_name = lc.name.0.as_str();
            s.push_str(&format!("{class_name}._fixtures_load_bang();\n"));
        }
    }

    EmittedFile {
        path: PathBuf::from("test/_runtime/setup.ts"),
        content: s,
    }
}


/// Emit a `LibraryClass` (a single class or mixin module from a
/// `runtime/ruby/*` file, with method signatures attached) as a
/// TypeScript class declaration — trailing newline included.
///
/// Surface choices:
///   * `parent: Some(StandardError)` → `extends Error` (TS's
///     equivalent). Other parents pass through verbatim.
///   * `parent: None` on a non-module → bare `class Foo`.
///   * `is_module: true` → bare `class Foo` for now (mixin semantics
///     are handled at the include site, not the definition site).
///   * Synthesized attr_reader pattern (zero-param method whose body
///     is `Ivar { name }` matching the method's own name) → emit as a
///     class field declaration; the read still works because callers
///     write `obj.foo` and TS resolves it to the field. Drops the
///     synthetic getter, which would have collided with the field.
///   * Synthesized attr_writer pattern (`name=` method that just
///     assigns the matching ivar) → drops likewise; the field
///     declaration above already supports `obj.foo = x`.
///   * `initialize` → `constructor`. Body uses TS's `this.x` for
///     ivars (already what `expr::emit_body` produces).
///   * `Class`-receiver methods → `static`.
///   * `include`s → emitted as a leading `// include: <Name>` comment;
///     real mixin support is deferred.
pub fn emit_library_class(class: &crate::dialect::LibraryClass) -> Result<String, String> {
    use crate::dialect::{AccessorKind, MethodReceiver};

    let class_name = class.name.0.as_str();
    let mut out = String::new();

    // Identify attribute readers/writers by the lowerer-recorded
    // `kind` field rather than pattern-matching the body — the
    // lowerer knows by construction (`synth_attr_reader`,
    // `synth_attr_writer`, `attr_*` ingest), so the IR carries the
    // fact directly. Restricted to instance receivers here because
    // class-receiver attribute accessors don't have an established
    // TS rendering pattern yet.
    let is_attr_reader = |m: &crate::dialect::MethodDef| -> bool {
        matches!(m.kind, AccessorKind::AttributeReader) && m.params.is_empty()
    };
    let is_attr_writer = |m: &crate::dialect::MethodDef| -> bool {
        matches!(m.kind, AccessorKind::AttributeWriter) && m.params.len() == 1
    };

    // Collect field declarations (from synthesized attr_readers — the
    // reader carries the type via its `() -> T` signature; body type
    // is the next-best source; final fallback is `any`). Class-level
    // attr_accessors (from `class << self; attr_accessor :x; end`)
    // become `static x: T;` field declarations; instance-level become
    // `x: T;`. Either form's setter is suppressed in favor of plain
    // assignment to the field.
    // (name, ty, is_static, from_ivar) — `from_ivar` distinguishes
    // ivar-assignment-derived fields (constructor body assigns
    // them) from attr_reader fields (typed accessors declared on
    // this class). When the class has a parent, ivar-derived
    // fields get a `declare` modifier — TS's signal that the
    // declaration is type-only and the property is provided by
    // the parent or by runtime assignment. Without `declare`, a
    // re-declared parent property trips TS2612.
    let mut fields: Vec<(String, String, bool, bool)> = Vec::new();
    let mut field_names_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in &class.methods {
        if is_attr_reader(m) {
            let ty = match m.signature.as_ref() {
                Some(Ty::Fn { ret, .. }) => ts_ty(ret),
                _ => m.body.ty.as_ref().map(ts_ty).unwrap_or_else(|| "any".to_string()),
            };
            let is_static = matches!(m.receiver, MethodReceiver::Class);
            field_names_seen.insert(m.name.as_str().to_string());
            fields.push((m.name.as_str().to_string(), ty, is_static, false));
        }
    }

    // Pre-walk every instance-method body looking for `@ivar = …`
    // assignments that aren't already declared as attr_readers.
    // TypeScript strict-mode requires field declarations for
    // `this.foo` writes; controllers (which assign action-locals like
    // `@article` for the view to read) and tests (which assign
    // fixture-helpers at setup time) are the producers. Type comes
    // from the assignment RHS; falls back to `any` when the analyzer
    // didn't infer one.
    let mut ivar_assignments: indexmap::IndexMap<String, Ty> = indexmap::IndexMap::new();
    let mut static_ivar_assignments: indexmap::IndexMap<String, Ty> = indexmap::IndexMap::new();
    for m in &class.methods {
        match m.receiver {
            MethodReceiver::Instance => {
                collect_ivar_assignments(&m.body, &mut ivar_assignments);
            }
            MethodReceiver::Class => {
                // `def self.reset_slots!; @slots = {}; end` —
                // module-level `@slots` is shared across all class
                // methods and emits as a `static` field.
                // module_function-promoted methods land here too,
                // so framework runtime files like `view_helpers.rb`
                // (which uses `@slots` from inside `module_function`
                // methods) get a static field declaration on the
                // emitted class.
                collect_ivar_assignments(&m.body, &mut static_ivar_assignments);
            }
        }
    }
    for (name, ty) in ivar_assignments {
        if field_names_seen.insert(name.clone()) {
            fields.push((name, ts_ty(&ty), false, true));
        }
    }
    for (name, ty) in static_ivar_assignments {
        if field_names_seen.insert(name.clone()) {
            fields.push((name, ts_ty(&ty), true, true));
        }
    }

    // Class header. Parent translation:
    //   - `StandardError` → `Error` (TS builtin)
    //   - `ActiveRecord::Base` → `ActiveRecordBase` (transpiled, aliased
    //     from `Base` in src/active_record_base.ts via render_imports).
    //     The juntos-side `ApplicationRecord` was a parallel
    //     hand-written re-implementation; this redirect makes the
    //     transpiled framework Ruby the single source of truth and
    //     forces juntos's surface down to the per-target primitive
    //     layer (`project_active_record_layering`).
    //   - Other qualified names: last segment (Ruby's `Foo::Bar` → TS
    //     `Bar` after import)
    // Modules emit as classes for now; include-as-mixin is deferred.
    let parent = class.parent.as_ref().map(|p| {
        let raw = p.0.as_str();
        match raw {
            "StandardError" => "Error".to_string(),
            "ActiveRecord::Base" => "ActiveRecordBase".to_string(),
            // Test parents — runtime adapter exports both names.
            "ActiveSupport::TestCase" | "ActionDispatch::IntegrationTest" => "TestCase".to_string(),
            "Minitest::Test" => "Test".to_string(),
            // Controller base — runtime exports `Base`; the import is
            // aliased in render_imports so the extends clause reads
            // `extends ActionControllerBase`.
            "ActionController::Base" | "ActionController::API" => {
                "ActionControllerBase".to_string()
            }
            _ => raw.rsplit("::").next().unwrap_or(raw).to_string(),
        }
    });
    // `include Mod` semantics: Ruby's mixin doesn't translate to TS
    // multiple inheritance. Single-include + no-parent collapses to
    // `extends <Mod>` so the included module's methods reach
    // subclasses through TS's inheritance chain. Other shapes
    // (include with explicit parent, multiple includes) still emit
    // a comment placeholder — they need the include-as-mixin pass.
    let synthesized_parent = if parent.is_none() && class.includes.len() == 1 {
        Some(class.includes[0].0.as_str().rsplit("::").next().unwrap().to_string())
    } else {
        None
    };
    let effective_parent = parent.as_deref().or(synthesized_parent.as_deref());
    match effective_parent {
        Some(p) => writeln!(out, "export class {class_name} extends {p} {{").unwrap(),
        None => writeln!(out, "export class {class_name} {{").unwrap(),
    }

    if synthesized_parent.is_none() && !class.includes.is_empty() {
        for inc in &class.includes {
            writeln!(out, "  // include: {}", inc.0.as_str()).unwrap();
        }
    }

    let mut wrote_fields = false;
    let has_parent = effective_parent.is_some();
    // Field names declared by the framework Base (active_record_base.ts)
    // — every model extends ActiveRecordBase transitively, so these
    // need a `declare` modifier on the subclass to avoid TS2612.
    // Hardcoded for the small known set rather than threading the
    // parent's field list through emit_library_class; expand when a
    // new framework parent surface materializes.
    const INHERITED_FIELD_NAMES: &[&str] = &["id", "errors", "persisted", "destroyed"];
    for (name, ty, is_static, from_ivar) in &fields {
        let prefix = if *is_static { "static " } else { "" };
        // ivar-derived fields on a derived class get `declare` —
        // the constructor body's `this.x = ...` (or a parent
        // declaration) provides the runtime backing; the field
        // line is type-only. attr_reader-derived fields get
        // `declare` only when the name matches a known
        // framework-inherited field (id/errors/persisted/destroyed)
        // since attr_readers also declare per-class fields for
        // schema columns the parent doesn't have (title, body).
        let inherited = INHERITED_FIELD_NAMES.contains(&name.as_str());
        let needs_declare = has_parent && (*from_ivar || inherited);
        let declare_modifier = if needs_declare { "declare " } else { "" };
        // Static fields synthesized from class-method `@ivar = ...`
        // assignments (module-level state in `module ViewHelpers;
        // @slots = {}; def self.reset_slots!; @slots = {}; end; end`)
        // need an initializer — without one, the field is `undefined`
        // until `reset_slots_bang()` is called, and any earlier read
        // (`this.slots[slot] = ...` from `content_for_set`) crashes.
        // The Ruby source initializes module-level @vars at module
        // load; mirror that with a type-driven default. Skip the
        // initializer when the field is `declare`d (parent provides
        // backing).
        let initializer = if *is_static && !needs_declare {
            ts_default_for_type(ty)
        } else {
            String::new()
        };
        writeln!(
            out,
            "  {prefix}{declare_modifier}{name}: {ty}{initializer};",
        ).unwrap();
        wrote_fields = true;
    }

    // `?`/`!` method-name suffixes get stripped on the way out
    // (`save!` → `save`, `valid?` → `valid`); when both forms exist
    // on the same class the sanitized names collide and TS rejects
    // the duplicate member. Drop the bang/predicate variant when a
    // plain-named twin exists — either as another method with the
    // same sanitized name (`save` vs `save!`) or as a field
    // declaration (`@persisted` ivar field collides with `persisted?`
    // sanitized to `persisted`). Predicate bodies that just read the
    // ivar (`def persisted?; @persisted; end`) are subsumed by the
    // field; callers reading `record.persisted?` sanitize to
    // `record.persisted` and get the field directly.
    let mut sanitized_seen: std::collections::HashSet<String> =
        field_names_seen.clone();
    for m in &class.methods {
        let raw = m.name.as_str();
        if !raw.ends_with('?') && !raw.ends_with('!') {
            sanitized_seen.insert(crate::emit::typescript::library::sanitize_identifier(raw));
        }
    }

    let methods_to_emit: Vec<&crate::dialect::MethodDef> = class
        .methods
        .iter()
        .filter(|m| !is_attr_reader(m) && !is_attr_writer(m))
        .filter(|m| {
            // Operator-method names (`[]`, `[]=`, `==`, …) aren't
            // valid TS method identifiers. TS lacks operator
            // overloading, so even renaming (`==` → `equals`) leaves
            // the bodies uncalled by the emitted code: comparison
            // sites lower to `===`, indexing lowers to `[]`, etc.
            // Skipping keeps the file syntactically valid; the
            // bodies remain readable in the source `.rb`.
            !matches!(
                m.name.as_str(),
                "[]" | "[]=" | "==" | "!=" | "<=>" | "<" | ">" | "<=" | ">="
                    | "<<" | ">>" | "+" | "-" | "*" | "/" | "%" | "**"
                    | "&" | "|" | "^" | "~" | "!" | "==="
            )
        })
        .filter(|m| {
            let raw = m.name.as_str();
            let stripped =
                crate::emit::typescript::library::sanitize_identifier(raw);
            // Drop the method whenever its sanitized name collides
            // with a field declaration (ivar OR attr_reader). The
            // common cases: `def errors; @errors ||= []; end`,
            // `def persisted?; @persisted; end` — bodies that just
            // accessor-expose an ivar are subsumed by the field.
            // Non-trivial colliders (rare) lose runtime semantics
            // here; surface those as a separate Ruby-source change
            // rather than emitting broken TS.
            if field_names_seen.contains(&stripped) {
                return false;
            }
            // Predicate/bang vs same-name plain-method twin (`save`
            // vs `save!`): keep the plain twin.
            if raw.ends_with('?') || raw.ends_with('!') {
                !sanitized_seen.contains(&stripped)
            } else {
                true
            }
        })
        .collect();

    if wrote_fields && !methods_to_emit.is_empty() {
        writeln!(out).unwrap();
    }

    let mut first = true;
    for m in methods_to_emit {
        if !first {
            writeln!(out).unwrap();
        }
        first = false;
        let body_str = emit_class_member(m, has_parent)?;
        for line in body_str.lines() {
            if line.is_empty() {
                writeln!(out).unwrap();
            } else {
                writeln!(out, "  {line}").unwrap();
            }
        }
    }

    out.push_str("}\n");
    Ok(out)
}

/// Emit the body of a `constructor` from an `initialize` method's
/// `Expr`. Floats top-level `super(...)` calls to the front so TS's
/// strict-derived-class rule (no `this` access before super) holds
/// even when the source Ruby wrote `@x = arg; super(...)`.
fn emit_constructor_body(
    body: &crate::expr::Expr,
    return_ty: &Ty,
    has_parent: bool,
) -> String {
    use crate::expr::{Expr, ExprNode};

    let exprs: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };

    let (supers, rest): (Vec<&Expr>, Vec<&Expr>) = exprs
        .into_iter()
        .partition(|e| matches!(*e.node, ExprNode::Super { .. }));

    if supers.is_empty() {
        // Derived classes (TS `class X extends Y`) require an explicit
        // `super(...)` call before any `this.*` access in the
        // constructor. Ruby's `def initialize` defaults to an
        // implicit `super` to the parent's `initialize`; for our
        // emit we materialize that as a synthetic zero-arg `super()`
        // so the TS strict-derived-class rule holds. Without this,
        // a class like `Base extends Validations` whose
        // `initialize` writes `@id = 0` trips TS17009 + TS2377.
        if has_parent {
            return format!("super();\n{}", expr::emit_body(body, return_ty));
        }
        return expr::emit_body(body, return_ty);
    }

    let mut reordered_exprs: Vec<Expr> = Vec::new();
    for s in supers {
        reordered_exprs.push((*s).clone());
    }
    for r in rest {
        reordered_exprs.push((*r).clone());
    }
    let reordered = Expr::new(body.span, ExprNode::Seq { exprs: reordered_exprs });
    expr::emit_body(&reordered, return_ty)
}

/// Walk an expression tree looking for `ExprNode::Yield`. Used to
/// detect methods that need a `__block` parameter injected — Ruby's
/// implicit-block yield translates to `__block(args)` in the emit,
/// so the method signature must declare the parameter for tsc to
/// resolve the name.
/// TS initializer fragment (` = <expr>`) for a field whose Ruby
/// equivalent would default to nil (unset @ivar reads as nil).
/// For Hash/Array, emit a fresh empty literal so reads don't crash
/// on `undefined.<key>` / `undefined.length`. Other concrete types
/// fall back to `null` as a Ruby-nil-aligned default. Untyped /
/// Var return empty (no initializer) — the consumer must guard.
fn ts_default_for_type(ty: &str) -> String {
    // String matching is the cheap path — `ts_ty` already collapsed
    // the Ty into its TS form, and the common cases are
    // string-distinguishable.
    if ty.starts_with("Record<") || ty == "any" {
        " = {}".to_string()
    } else if ty.ends_with("[]") || ty.starts_with("Array<") {
        " = []".to_string()
    } else if ty == "string" {
        " = \"\"".to_string()
    } else if ty == "number" {
        " = 0".to_string()
    } else if ty == "boolean" {
        " = false".to_string()
    } else {
        // Class types, unions, etc. — leave uninitialized so tsc
        // doesn't infer `null` into a non-nullable position.
        // Static-field-from-class-method usage reassigns before
        // reading anyway in the framework patterns we ship today.
        String::new()
    }
}

fn body_contains_yield(body: &crate::expr::Expr) -> bool {
    use crate::expr::{ExprNode, LValue};
    match &*body.node {
        ExprNode::Yield { .. } => true,
        ExprNode::Seq { exprs } => exprs.iter().any(body_contains_yield),
        ExprNode::If { cond, then_branch, else_branch } => {
            body_contains_yield(cond)
                || body_contains_yield(then_branch)
                || body_contains_yield(else_branch)
        }
        ExprNode::Case { scrutinee, arms } => {
            body_contains_yield(scrutinee)
                || arms.iter().any(|a| {
                    a.guard.as_ref().is_some_and(body_contains_yield)
                        || body_contains_yield(&a.body)
                })
        }
        ExprNode::Send { recv, args, block, .. } => {
            recv.as_ref().is_some_and(body_contains_yield)
                || args.iter().any(body_contains_yield)
                || block.as_ref().is_some_and(body_contains_yield)
        }
        ExprNode::Apply { fun, args, block } => {
            body_contains_yield(fun)
                || args.iter().any(body_contains_yield)
                || block.as_ref().is_some_and(body_contains_yield)
        }
        ExprNode::Lambda { body: lb, .. } => body_contains_yield(lb),
        ExprNode::Assign { target, value } => {
            (match target {
                LValue::Attr { recv, .. } | LValue::Index { recv, .. } => {
                    body_contains_yield(recv)
                }
                _ => false,
            }) || body_contains_yield(value)
        }
        ExprNode::Return { value } => body_contains_yield(value),
        ExprNode::Raise { value } => body_contains_yield(value),
        ExprNode::Next { value } => value.as_ref().is_some_and(body_contains_yield),
        ExprNode::Super { args } => args
            .as_ref()
            .is_some_and(|v| v.iter().any(body_contains_yield)),
        ExprNode::BoolOp { left, right, .. } => {
            body_contains_yield(left) || body_contains_yield(right)
        }
        ExprNode::While { cond, body: wb, .. } => {
            body_contains_yield(cond) || body_contains_yield(wb)
        }
        ExprNode::RescueModifier { expr, fallback } => {
            body_contains_yield(expr) || body_contains_yield(fallback)
        }
        ExprNode::BeginRescue {
            body: inner,
            rescues,
            else_branch,
            ensure,
            ..
        } => {
            body_contains_yield(inner)
                || rescues.iter().any(|r| body_contains_yield(&r.body))
                || else_branch.as_ref().is_some_and(body_contains_yield)
                || ensure.as_ref().is_some_and(body_contains_yield)
        }
        ExprNode::Range { begin, end, .. } => {
            begin.as_ref().is_some_and(body_contains_yield)
                || end.as_ref().is_some_and(body_contains_yield)
        }
        ExprNode::MultiAssign { value, .. } => body_contains_yield(value),
        ExprNode::StringInterp { parts } => {
            parts.iter().any(|p| match p {
                crate::expr::InterpPart::Expr { expr } => body_contains_yield(expr),
                crate::expr::InterpPart::Text { .. } => false,
            })
        }
        ExprNode::Array { elements, .. } => elements.iter().any(body_contains_yield),
        ExprNode::Hash { entries, .. } => entries
            .iter()
            .any(|(k, v)| body_contains_yield(k) || body_contains_yield(v)),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => false,
        // Catch-all for any new ExprNode variants — be conservative
        // and assume "no yield" so the function still compiles. New
        // nodes that wrap sub-expressions should add their own arm.
        _ => false,
    }
}

/// Emit one `MethodDef` as a class member (instance method, static,
/// or constructor). Uses signature when present (typed params + ret);
/// falls back to body.ty for return and `any` for params when not
/// (lowered models don't populate signatures yet).
fn emit_class_member(
    m: &crate::dialect::MethodDef,
    has_parent: bool,
) -> Result<String, String> {
    use crate::dialect::MethodReceiver;

    // Pull (param-types, kinds, return-type) from signature when
    // available. Kinds drive optional-param decoration: Ruby kwargs
    // with defaults (`def foo(x, status: 200)`) and explicit-optional
    // positionals (`def foo(x = nil)`) emit as TS `name?: T` so
    // call sites that omit them type-check. Without this, every
    // kwarg-default call (`render(html)` where Ruby has
    // `render(html, status: 200)`) trips TS2554.
    //
    // `is_keyword` per param drives the destructured-object emit at
    // the end: kwargs from Ruby (`def x(a:, b: 0)`) become a single
    // trailing `{a, b}: {a: T, b?: U}` destructured object so call
    // sites that pass a Hash literal (`x({a: 1})`) match.
    let (sig_param_tys, sig_param_optional, sig_param_is_keyword, ret_ty): (
        Vec<Ty>,
        Vec<bool>,
        Vec<bool>,
        Ty,
    ) = match m.signature.as_ref() {
        Some(Ty::Fn { params: sig_params, ret, .. }) => {
            let non_block: Vec<&crate::ty::Param> = sig_params
                .iter()
                .filter(|p| !matches!(p.kind, crate::ty::ParamKind::Block))
                .collect();
            if non_block.len() != m.params.len() {
                return Err(format!(
                    "method `{}`: signature/param arity mismatch ({} vs {})",
                    m.name,
                    non_block.len(),
                    m.params.len(),
                ));
            }
            let tys = non_block.iter().map(|p| p.ty.clone()).collect();
            let optionals = non_block
                .iter()
                .map(|p| {
                    matches!(
                        p.kind,
                        crate::ty::ParamKind::Optional
                            | crate::ty::ParamKind::Keyword { required: false }
                            | crate::ty::ParamKind::KeywordRest
                    )
                })
                .collect();
            let is_keyword = non_block
                .iter()
                .map(|p| {
                    matches!(
                        p.kind,
                        crate::ty::ParamKind::Keyword { .. } | crate::ty::ParamKind::KeywordRest
                    )
                })
                .collect();
            (tys, optionals, is_keyword, (**ret).clone())
        }
        _ => (
            m.params.iter().map(|_| Ty::Untyped).collect(),
            m.params.iter().map(|_| false).collect(),
            m.params.iter().map(|_| false).collect(),
            m.body.ty.clone().unwrap_or(Ty::Nil),
        ),
    };

    // Build the param list — positional params first (one slot each),
    // then a single destructured object holding any kwargs. Without
    // this, Ruby `fill_timestamps(creating: true)` call sites emit
    // `fill_timestamps({creating: true})` (Hash literal) but the def
    // signature is `fill_timestamps(creating: boolean)` (positional)
    // → TS2345 "argument of type {creating: boolean} not assignable
    // to parameter of type boolean".
    let mut param_slots: Vec<String> = Vec::new();
    // (name, ts_ty, optional, default_expr_str)
    let mut kwarg_pieces: Vec<(String, String, bool, Option<String>)> = Vec::new();
    for (i, name) in m.params.iter().enumerate() {
        let ty = &sig_param_tys[i];
        let optional = sig_param_optional[i];
        let is_kw = sig_param_is_keyword[i];
        if is_kw {
            // Carry through the default Expr (rendered as TS) when
            // the Ruby source supplied one — `def redirect_to(...
            // status: :found)` defaults `status` to "found", so a
            // call without `status:` resolves to 302 (Found) not 200.
            // Without this, the destructuring pattern uses the
            // generic `null` fallback and breaks every Rails
            // optional-kwarg-with-non-nil-default API.
            let default_s = name.default.as_ref().map(|d| expr::emit_expr(d));
            kwarg_pieces.push((name.as_str().to_string(), ts_ty(ty), optional, default_s));
        } else {
            // Default-value path: when the param is `Optional` AND
            // carries a default Expr, prefer `name: T = <default>`
            // over `name?: T`. The latter binds the param to
            // `undefined` when the caller omits it; the former
            // gives back the actual default the Ruby source wrote.
            // Matters for `def initialize(attrs = {})`: with `?:`
            // the body's `attrs["id"]` crashes on a no-args call
            // (`new Article()` from `from_row`), with `= {}` the
            // empty hash is what's read from. Both signatures
            // type-check at call sites since `?` and `=` give the
            // caller the same option to omit the argument.
            if optional && name.default.is_some() {
                let default_s = expr::emit_expr(name.default.as_ref().unwrap());
                param_slots.push(format!(
                    "{}: {} = {}",
                    escape_reserved(name.as_str()),
                    ts_ty(ty),
                    default_s,
                ));
            } else {
                let opt_marker = if optional { "?" } else { "" };
                param_slots.push(format!(
                    "{}{}: {}",
                    escape_reserved(name.as_str()),
                    opt_marker,
                    ts_ty(ty),
                ));
            }
        }
    }
    let body_uses_yield = body_contains_yield(&m.body);
    if !kwarg_pieces.is_empty() {
        // Each kwarg name appears in two slots: the destructuring
        // pattern (must be a valid binding) and the type annotation
        // (the original Ruby symbol, callers spell it that way).
        // When the Ruby name shadows a TS reserved word (`with`,
        // `class`, `default`), rename in the pattern via `:`-rename
        // and use the escaped local in the body. The body emit
        // already escapes via `escape_reserved` when reading the
        // local, so the pattern's `original: escaped` keeps the
        // type annotation untouched while the binding is JS-legal.
        // Optional kwargs default to `null` in the destructuring
        // pattern so omitted args produce `null` (matching Ruby's
        // `nil` default) rather than `undefined`. Without this,
        // `x.nil?` (which the emit lowers to `x === null`) returns
        // false for omitted kwargs — a Ruby `if !x.nil? && cond`
        // would pull in untaken branches with undefined values.
        let names: Vec<String> = kwarg_pieces
            .iter()
            .map(|(n, _, opt, default_s)| {
                let escaped = escape_reserved(n);
                let pat = if escaped == *n {
                    n.clone()
                } else {
                    format!("{n}: {escaped}")
                };
                match (opt, default_s) {
                    (true, Some(d)) => format!("{pat} = {d}"),
                    (true, None) => format!("{pat} = null"),
                    _ => pat,
                }
            })
            .collect();
        let typed: Vec<String> = kwarg_pieces
            .iter()
            .map(|(n, t, opt, _)| {
                let marker = if *opt { "?" } else { "" };
                format!("{n}{marker}: {t}")
            })
            .collect();
        // If every kwarg is optional, the kwarg object itself is
        // optional — call sites omitting kwargs entirely
        // (`fill_timestamps()`) still type-check. TS forbids `?` on
        // a destructuring binding pattern in an implementation
        // signature, so spell the optional via `= {}` default.
        let default_clause = if kwarg_pieces.iter().all(|(_, _, opt, _)| *opt) {
            " = {}"
        } else {
            ""
        };
        param_slots.push(format!(
            "{{ {} }}: {{ {} }}{}",
            names.join(", "),
            typed.join(", "),
            default_clause,
        ));
    }
    // Inject a `__block: (...args: any[]) => any` parameter when the
    // method body uses `yield`. The yield-emit code in expr.rs
    // produces `__block(args)`; without a corresponding parameter
    // declaration, tsc errors with "Cannot find name '__block'".
    // Block-aware call sites (`emit_send_with_block`) pass the block
    // as the trailing positional arg, so the wire-up works once
    // both ends agree on the param name. Goes LAST in the param
    // list — Ruby blocks always trail positional + keyword args.
    if body_uses_yield {
        param_slots.push("__block: (...args: any[]) => any".to_string());
    }
    let param_list = param_slots;

    let mut out = String::new();
    let raw_name = m.name.as_str();
    let mname = crate::emit::typescript::library::sanitize_identifier(raw_name);
    let is_constructor =
        raw_name == "initialize" && matches!(m.receiver, MethodReceiver::Instance);

    let rewritten = if is_constructor {
        crate::emit::typescript::library::rewrite_for_constructor(&m.body)
    } else {
        crate::emit::typescript::library::rewrite_for_class_method(&m.body, raw_name)
    };

    let body = if is_constructor {
        // TS constructors implicitly return the constructed instance;
        // an explicit `return <expr>` on the last statement of a Ruby
        // `def initialize` would replace `this` with the expression's
        // value (e.g. `return this.errors = []` → constructor returns
        // an array, breaking `instanceof` and method dispatch). Emit
        // body as void so the trailing-statement-becomes-return
        // transform is suppressed.
        emit_constructor_body(&rewritten, &Ty::Nil, has_parent)
    } else {
        expr::emit_body(&rewritten, &ret_ty)
    };

    if is_constructor {
        writeln!(out, "constructor({}) {{", param_list.join(", ")).unwrap();
    } else {
        let prefix = if matches!(m.receiver, MethodReceiver::Class) {
            "static "
        } else {
            ""
        };
        let ret_s = ts_return_ty(&ret_ty);
        writeln!(
            out,
            "{prefix}{}({}): {} {{",
            mname,
            param_list.join(", "),
            ret_s
        )
        .unwrap();
    }
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("}\n");
    Ok(out)
}

/// Emit a `LibraryFunction` as a top-level `export function` (no
/// surrounding class). Body emission shares the param-typing /
/// return-typing / body-typing machinery with `emit_class_member`,
/// but the rewrite pass differs: free functions don't have `this`,
/// so bare Sends and Ivar references aren't injected with SelfRef.
pub fn emit_library_function(
    func: &crate::dialect::LibraryFunction,
) -> Result<String, String> {
    let (sig_param_tys, sig_param_optional, ret_ty): (Vec<Ty>, Vec<bool>, Ty) =
        match func.signature.as_ref() {
            Some(Ty::Fn { params: sig_params, ret, .. }) => {
                let non_block: Vec<&crate::ty::Param> = sig_params
                    .iter()
                    .filter(|p| !matches!(p.kind, crate::ty::ParamKind::Block))
                    .collect();
                if non_block.len() != func.params.len() {
                    return Err(format!(
                        "function `{}`: signature/param arity mismatch ({} vs {})",
                        func.name,
                        non_block.len(),
                        func.params.len(),
                    ));
                }
                let tys = non_block.iter().map(|p| p.ty.clone()).collect();
                let optionals = non_block
                    .iter()
                    .map(|p| {
                        matches!(
                            p.kind,
                            crate::ty::ParamKind::Optional
                                | crate::ty::ParamKind::Keyword { required: false }
                                | crate::ty::ParamKind::KeywordRest
                        )
                    })
                    .collect();
                (tys, optionals, (**ret).clone())
            }
            _ => (
                func.params.iter().map(|_| Ty::Untyped).collect(),
                func.params.iter().map(|_| false).collect(),
                func.body.ty.clone().unwrap_or(Ty::Nil),
            ),
        };

    let param_list: Vec<String> = func
        .params
        .iter()
        .zip(sig_param_tys.iter())
        .zip(sig_param_optional.iter())
        .map(|((name, ty), optional)| {
            let opt_marker = if *optional { "?" } else { "" };
            format!(
                "{}{}: {}",
                escape_reserved(name.as_str()),
                opt_marker,
                ts_ty(ty)
            )
        })
        .collect();

    let raw_name = func.name.as_str();
    let mname = escape_for_function_name(raw_name);

    // Free-function rewrite: no SelfRef injection, no super rewrite —
    // bare Sends emit as plain function calls (resolved against
    // imports), and `super` doesn't apply since there's no inheritance.
    let rewritten = crate::emit::typescript::library::rewrite_for_free_function(&func.body);
    let body = expr::emit_body(&rewritten, &ret_ty);

    let ret_s = ts_return_ty(&ret_ty);
    let mut out = String::new();
    writeln!(
        out,
        "export function {}({}): {} {{",
        mname,
        param_list.join(", "),
        ret_s
    )
    .unwrap();
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("}\n");
    Ok(out)
}

/// Emit a list of typed `MethodDef`s — produced by
/// `parse_methods_with_rbs` from a whole `.rb` + `.rbs` pair — as a
/// single TypeScript module file (trailing newline included).
pub fn emit_module(methods: &[crate::dialect::MethodDef]) -> Result<String, String> {
    use crate::dialect::MethodReceiver;

    if methods.is_empty() {
        return Ok(String::new());
    }
    if !methods.iter().all(|m| matches!(m.receiver, MethodReceiver::Class)) {
        return Err(format!(
            "emit_module: only all-class-method modules supported so far; \
             saw mixed/instance methods (first instance: `{}`)",
            methods
                .iter()
                .find(|m| matches!(m.receiver, MethodReceiver::Instance))
                .map(|m| m.name.as_str())
                .unwrap_or("<none>"),
        ));
    }

    let mut out = String::new();
    for (i, m) in methods.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&emit_method(m));
    }
    Ok(out)
}

/// Map a Ruby identifier to a safe TS parameter name. Each name in
/// Identifier escape applied to LibraryFunction names. Strips Ruby's
/// `?`/`!` suffixes via `sanitize_identifier`, then maps reserved
/// JS words (`new`, `default`, etc.) to a `name_` suffix form so the
/// emitted `export function <x>` parses.
pub(super) fn escape_for_function_name(raw: &str) -> String {
    escape_reserved(&crate::emit::typescript::library::sanitize_identifier(raw))
}

/// Walk an Expr collecting every `@ivar = value` assignment, keyed
/// by the ivar name. Later assignments overwrite earlier ones (keeps
/// the most-narrowed type when the body assigns the same ivar
/// multiple places). Used by `emit_library_class` to synthesize
/// `name: type;` field declarations for ivars that aren't otherwise
/// declared via attr_reader.
fn collect_ivar_assignments(
    e: &crate::expr::Expr,
    out: &mut indexmap::IndexMap<String, Ty>,
) {
    use crate::expr::{ExprNode, InterpPart, LValue};
    match &*e.node {
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            // Type from the RHS, falling back to `any` (Ty::Untyped)
            // when the analyzer didn't infer one.
            let ty = value.ty.clone().unwrap_or(Ty::Untyped);
            out.insert(name.as_str().to_string(), ty);
            collect_ivar_assignments(value, out);
        }
        ExprNode::Assign { target, value } => {
            if let LValue::Attr { recv, .. } | LValue::Index { recv, .. } = target {
                collect_ivar_assignments(recv, out);
            }
            collect_ivar_assignments(value, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_ivar_assignments(r, out);
            }
            for a in args {
                collect_ivar_assignments(a, out);
            }
            if let Some(b) = block {
                collect_ivar_assignments(b, out);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            collect_ivar_assignments(fun, out);
            for a in args {
                collect_ivar_assignments(a, out);
            }
            if let Some(b) = block {
                collect_ivar_assignments(b, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_ivar_assignments(k, out);
                collect_ivar_assignments(v, out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                collect_ivar_assignments(el, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    collect_ivar_assignments(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_ivar_assignments(left, out);
            collect_ivar_assignments(right, out);
        }
        ExprNode::Let { value, body, .. } => {
            collect_ivar_assignments(value, out);
            collect_ivar_assignments(body, out);
        }
        ExprNode::Lambda { body, .. } => collect_ivar_assignments(body, out),
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_ivar_assignments(cond, out);
            collect_ivar_assignments(then_branch, out);
            collect_ivar_assignments(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_ivar_assignments(scrutinee, out);
            for arm in arms {
                collect_ivar_assignments(&arm.body, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for sub in exprs {
                collect_ivar_assignments(sub, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                collect_ivar_assignments(a, out);
            }
        }
        ExprNode::Raise { value } => collect_ivar_assignments(value, out),
        ExprNode::RescueModifier { expr, fallback } => {
            collect_ivar_assignments(expr, out);
            collect_ivar_assignments(fallback, out);
        }
        ExprNode::Return { value } => collect_ivar_assignments(value, out),
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_ivar_assignments(body, out);
            for r in rescues {
                collect_ivar_assignments(&r.body, out);
            }
            if let Some(eb) = else_branch {
                collect_ivar_assignments(eb, out);
            }
            if let Some(ensure_b) = ensure {
                collect_ivar_assignments(ensure_b, out);
            }
        }
        ExprNode::Next { value } => {
            if let Some(v) = value {
                collect_ivar_assignments(v, out);
            }
        }
        ExprNode::MultiAssign { value, .. } => collect_ivar_assignments(value, out),
        ExprNode::While { cond, body, .. } => {
            collect_ivar_assignments(cond, out);
            collect_ivar_assignments(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                collect_ivar_assignments(b, out);
            }
            if let Some(e2) = end {
                collect_ivar_assignments(e2, out);
            }
        }
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    collect_ivar_assignments(a, out);
                }
            }
        }
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
    }
}

/// the list below is reserved in TS but commonly used as a Rails-side
/// method/keyword arg.
fn escape_reserved(name: &str) -> String {
    matches!(
        name,
        "default"
            | "with"
            | "function"
            | "class"
            | "for"
            | "let"
            | "const"
            | "var"
            | "return"
            | "switch"
            | "case"
            | "if"
            | "else"
            | "while"
            | "do"
            | "yield"
            | "delete"
            | "new"
            | "this"
            | "super"
            | "true"
            | "false"
            | "null"
            | "void"
            | "typeof"
            | "instanceof"
    )
    .then(|| format!("{name}_"))
    .unwrap_or_else(|| name.to_string())
}

/// Emit a typed `MethodDef` as a standalone exported TypeScript
/// function (trailing newline included). Used by the
/// runtime-extraction pipeline.
pub fn emit_method(m: &crate::dialect::MethodDef) -> String {
    let sig = m
        .signature
        .as_ref()
        .expect("emit_method requires a signature");
    let Ty::Fn { params: sig_params, ret, .. } = sig else {
        panic!("signature is not Ty::Fn");
    };
    assert_eq!(
        sig_params.len(),
        m.params.len(),
        "method `{}`: signature/param arity mismatch",
        m.name
    );

    let param_list: Vec<String> = m
        .params
        .iter()
        .zip(sig_params.iter())
        .map(|(name, p)| format!("{}: {}", name, ts_ty(&p.ty)))
        .collect();

    let ret_s = ts_return_ty(ret);
    let body = expr::emit_body(&m.body, ret);

    let mut out = String::new();
    writeln!(
        out,
        "export function {}({}): {} {{",
        m.name,
        param_list.join(", "),
        ret_s
    )
    .unwrap();
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("}\n");
    out
}
