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
// Minitest-shaped Crystal Test base class (`RoundhouseTest`). Emitted
// as `src/test_helper.cr` whenever the App carries test_modules.
const CR_TEST_HELPER_SOURCE: &str = include_str!("../../runtime/crystal/test_helper.cr");
// In-memory adapter mirroring `runtime/ruby/test/test_helper.rb`'s
// `FrameworkTestAdapter` — emitted as `src/framework_test_adapter.cr`
// whenever test_modules are present, and required from app.cr after
// db.cr (depends on `Roundhouse::ActiveRecordAdapter`).
const CR_FRAMEWORK_TEST_ADAPTER_SOURCE: &str =
    include_str!("../../runtime/crystal/framework_test_adapter.cr");

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
    // class registry. Html-only — jbuilder (json-format) views go
    // through `lower_jbuilder_to_library_classes` below.
    let mut view_lower_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.clone().into_iter().collect();
    view_lower_extras.extend(route_helper_extras.clone());
    let view_lcs =
        crate::lower::lower_views_to_library_classes(&app.views, app, view_lower_extras.clone());
    let jbuilder_lcs =
        crate::lower::lower_jbuilder_to_library_classes(&app.views, app, view_lower_extras);

    // Controllers → src/controllers/<stem>.cr; synthesized
    // `<Resource>Params` siblings route to src/models/.
    let mut controller_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    controller_extras.extend(crate::lower::extras_from_lcs(&view_lcs));
    controller_extras.extend(route_helper_extras);
    let controller_lcs = crate::lower::lower_controllers_with_arel_and_views(
        &app.controllers,
        controller_extras,
        Some(&app.schema),
        &app.views,
    );
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
    // type. Pair them with the corresponding html-format `View` so we
    // get the right output path; jbuilder (json) views fan out in the
    // companion loop below.
    let html_views: Vec<&crate::dialect::View> =
        app.views.iter().filter(|v| v.format.as_str() == "html").collect();
    for (v, lc) in html_views.iter().zip(view_lcs.iter()) {
        let out_path = view_output_path(v.name.as_str());
        files.push(library::emit_library_class_decl(lc, app, out_path));
    }

    // Jbuilder views → src/views/<dir>/<base>_json.cr. Reopens the
    // same `Views::<Plural>` module its html sibling defines, adding
    // `<base>_json(arg)` methods returning a JSON string. The `_json`
    // suffix prevents path collision with the html sibling.
    let json_views: Vec<&crate::dialect::View> =
        app.views.iter().filter(|v| v.format.as_str() == "json").collect();
    for (v, lc) in json_views.iter().zip(jbuilder_lcs.iter()) {
        let out_path = jbuilder_view_output_path(v.name.as_str());
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

    // Test modules — one `spec/<stem>_spec.cr` per ingested
    // TestModule, plus `src/test_helper.cr` (the `RoundhouseTest`
    // Minitest analog) when any tests are present. Inner classes
    // (`class Validatable; …; end` declared inside the test class)
    // and class-body constants (`TABLE = […]`) hoist to file scope
    // above the test class — same pattern as the Ruby/Spinel emit
    // (see `src/emit/ruby.rs::emit_spinel`). Crystal's compiler does
    // whole-program analysis on the spec build, so per-spec require
    // headers aren't needed beyond the test_helper itself.
    if !app.test_modules.is_empty() {
        use std::fmt::Write as _;
        files.push(EmittedFile {
            path: PathBuf::from("src/test_helper.cr"),
            content: CR_TEST_HELPER_SOURCE.to_string(),
        });
        // In-memory adapter for framework-level tests. Loaded via
        // app.cr's alphabetical sweep (`framework_test_adapter`
        // comes after `db` alphabetically, so the abstract base it
        // inherits from is already defined when this file parses).
        files.push(EmittedFile {
            path: PathBuf::from("src/framework_test_adapter.cr"),
            content: CR_FRAMEWORK_TEST_ADAPTER_SOURCE.to_string(),
        });
        // Framework runtime RBS — translate each `(class, method →
        // Ty)` row into a ClassInfo extra so the test body-typer
        // dispatches precisely against framework methods. Same
        // pattern the TS emit uses (see typescript.rs); critical
        // for Crystal because `Ty::Untyped` falls through to
        // `String` at the emit boundary, so RBS-driven typing of
        // the test body is what produces correct typed dispatch
        // (e.g. NamedTuple receivers from typed Router.match).
        let mut test_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> = Vec::new();
        for (class_id, methods) in &app.rbs_signatures {
            let mut info = crate::analyze::ClassInfo::default();
            for (m_name, m_ty) in methods {
                info.instance_methods.insert(m_name.clone(), m_ty.clone());
            }
            test_extras.push((class_id.clone(), info));
        }
        let test_lowered = crate::lower::lower_test_modules_with_inner(
            &app.test_modules,
            &app.fixtures,
            &app.models,
            test_extras,
        );
        for lowered in &test_lowered {
            let lc = &lowered.test_class;
            let class_name = lc.name.0.as_str();
            let stem = crate::naming::snake_case(
                class_name.strip_suffix("Test").unwrap_or(class_name),
            );
            let out_path = PathBuf::from(format!("spec/{stem}_spec.cr"));
            let mut content = String::new();
            content.push_str("require \"../src/test_helper\"\n");
            content.push_str("require \"../src/app\"\n\n");
            // Hoist class-body constants to file scope first — Crystal
            // top-level constants are visible everywhere below them,
            // mirroring the Ruby `TABLE = [...]` lift in spinel emit.
            // `.freeze` calls strip out: Crystal Array/Hash literals
            // are mutable by default and `.freeze` has no analog in
            // the Crystal API; Ruby's `freeze` was protective, not
            // semantic to the test.
            for (name, value) in &lowered.constants {
                let value_expr: &crate::expr::Expr = match &*value.node {
                    crate::expr::ExprNode::Send { recv: Some(r), method, args, .. }
                        if method.as_str() == "freeze" && args.is_empty() => r,
                    _ => value,
                };
                // `NAME = Struct.new(:a, :b, :c) do ... end` → synthesize
                // a Crystal class with positional constructor params
                // bound to public properties. Mirrors the same lift in
                // the TS emit; Crystal property declarations carry a
                // type, so default to `String` (the most common case
                // for the framework tests' Struct stand-ins; refine to
                // typed fields when the body-typer knows better).
                if let crate::expr::ExprNode::Send { recv, method, args, .. } = &*value_expr.node {
                    let recv_is_struct = recv.as_ref().is_some_and(|r| {
                        matches!(&*r.node, crate::expr::ExprNode::Const { path }
                            if path.len() == 1 && path[0].as_str() == "Struct")
                    });
                    if recv_is_struct && method.as_str() == "new" {
                        let fields: Vec<&str> = args
                            .iter()
                            .filter_map(|a| match &*a.node {
                                crate::expr::ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => Some(value.as_str()),
                                _ => None,
                            })
                            .collect();
                        if !fields.is_empty() {
                            writeln!(content, "class {}", name.as_str()).unwrap();
                            for f in &fields {
                                writeln!(content, "  property {f} : (String | Int32 | Nil)").unwrap();
                            }
                            write!(content, "  def initialize(").unwrap();
                            let params = fields
                                .iter()
                                .map(|f| format!("@{f} : (String | Int32 | Nil) = nil"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            content.push_str(&params);
                            writeln!(content, ")").unwrap();
                            writeln!(content, "  end").unwrap();
                            // `record[:field]` → `record.get(:field)` — the
                            // Crystal emit's framework-class bracket-access
                            // doesn't know about our Struct stand-ins, so
                            // give them a `[]` accessor that maps to the
                            // matching property.
                            writeln!(content, "  def [](field : Symbol)").unwrap();
                            writeln!(content, "    case field").unwrap();
                            for f in &fields {
                                writeln!(content, "    when :{f} then @{f}").unwrap();
                            }
                            writeln!(content, "    end").unwrap();
                            writeln!(content, "  end").unwrap();
                            writeln!(content, "end").unwrap();
                            continue;
                        }
                    }
                }
                // Array-of-Hash-literals where every Hash has all
                // Symbol-literal keys → emit as Array-of-NamedTuple
                // so Crystal's type system sees a fixed-shape record.
                // Used by test fixtures like router_test's TABLE
                // constant; matches the typed `Array[{method:, ...}]`
                // signature on `Router.match`. Hash form would
                // produce `Array(Hash(Symbol, V))` which the typed
                // signature rejects.
                if let crate::expr::ExprNode::Array { elements, .. } = &*value_expr.node {
                    let all_named_hashes = !elements.is_empty()
                        && elements.iter().all(|el| {
                            matches!(&*el.node, crate::expr::ExprNode::Hash { entries, .. }
                                if !entries.is_empty()
                                    && entries.iter().all(|(k, _)| matches!(&*k.node,
                                        crate::expr::ExprNode::Lit { value: crate::expr::Literal::Sym { .. } })))
                        });
                    if all_named_hashes {
                        let parts: Vec<String> = elements
                            .iter()
                            .map(|el| {
                                let crate::expr::ExprNode::Hash { entries, .. } = &*el.node else {
                                    unreachable!()
                                };
                                let inner: Vec<String> = entries
                                    .iter()
                                    .map(|(k, v)| {
                                        let crate::expr::ExprNode::Lit { value: crate::expr::Literal::Sym { value } } = &*k.node else {
                                            unreachable!()
                                        };
                                        format!("{}: {}", value.as_str(), expr::emit_expr(v))
                                    })
                                    .collect();
                                format!("{{{}}}", inner.join(", "))
                            })
                            .collect();
                        writeln!(content, "{} = [{}]", name.as_str(), parts.join(", ")).unwrap();
                        continue;
                    }
                }
                let value_s = expr::emit_expr(value_expr);
                writeln!(content, "{} = {}", name.as_str(), value_s).unwrap();
            }
            if !lowered.constants.is_empty() {
                content.push('\n');
            }
            // Inner classes hoist next, above the test class proper.
            for inner in &lowered.inner_classes {
                content.push_str(&library::emit_library_class_decl(inner, app, PathBuf::new()).content);
                content.push('\n');
            }
            content.push_str(&library::emit_library_class_decl(lc, app, out_path.clone()).content);
            files.push(EmittedFile {
                path: out_path,
                content,
            });
        }
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
/// runtime has known dependencies (action_controller_base may include
/// modules); hardcoding the order keeps things deterministic.
fn emit_app_cr(emitted: &[EmittedFile]) -> EmittedFile {
    // Framework runtime files in dependency order. Each must be
    // loaded before any file that `include`s its module.
    const RUNTIME_ORDER: &[&str] = &[
        "hash_with_indifferent_access",
        "inflector",
        // active_record_base → errors keeps the forward-ref chain
        // resolvable at parse time. Base is referenced by errors'
        // RecordInvalid as a property type (parse-time class macro);
        // errors is used by Base's body `raise RecordNotFound/
        // RecordInvalid` (lazy method-body resolution).
        "active_record_base",
        "errors",
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

/// Jbuilder counterpart: `articles/_article` → `src/views/articles/
/// _article_json.cr`. Matches the lowered method name (`article_json`)
/// and keeps the html sibling's file slot free.
fn jbuilder_view_output_path(view_name: &str) -> PathBuf {
    PathBuf::from(format!("src/views/{view_name}_json.cr"))
}
