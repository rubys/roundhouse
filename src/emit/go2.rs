//! Go target — `go2` parallel emit (Phase 1 scaffold).
//!
//! Mirrors the `rust2` migration pattern. Strangler-fig orchestrator
//! that runs alongside the legacy `src/emit/go.rs` while the migration
//! to Group 1 (lowered IR + transpiled `runtime/ruby/`) lands.
//!
//! Selected at runtime via `ROUNDHOUSE_GO_V2=1`. Without the env var,
//! `super::go::emit` runs unchanged and emits the same output it
//! always has. With the env var, this module's overlay runs after
//! the legacy emit and writes transpiled framework runtime files
//! into the output project under `app/v2/` (separate Go package, so
//! emission can't conflict with the hand-written runtime types).
//!
//! Phase 1 scope: scaffold + minimal transpile via stub
//! `library::emit_library_class` (see `library.rs` for the contract).
//! Transpiled files are emitted but their method bodies are
//! `panic("go2 stub")` — `go build ./app/v2/...` produces a real
//! error inventory we can drive future sessions against.
//!
//! Out of scope for Phase 1: replacing the hand-written
//! `runtime/go/*.go` files (cable, http, server, view_helpers,
//! runtime, test_support, db) with transpiled equivalents. Those
//! land once the per-method body emit is real enough that calls
//! through them survive `go build`.

use std::path::PathBuf;

use super::EmittedFile;
use crate::App;

mod expr;
mod library;
pub mod lower;
mod ty;

/// Phase 3 hand-written primitive runtime. Verbatim copy into the
/// v2/ overlay — these files declare `package v2` already so no
/// rewriting needed. They land alongside the transpiled framework
/// runtime so cross-file references (the transpiled `ActiveRecord`
/// module-singleton's `*AdapterInterface` slot type) resolve at
/// `go vet` / `go build` time.
const RT_V2_ADAPTER_INTERFACE: &str =
    include_str!("../../runtime/go/v2/adapter_interface.go");
const RT_V2_PARAM_VALUE: &str =
    include_str!("../../runtime/go/v2/param_value.go");
const RT_V2_DB: &str =
    include_str!("../../runtime/go/v2/db.go");
const RT_V2_BROADCASTS: &str =
    include_str!("../../runtime/go/v2/broadcasts.go");
const RT_V2_SERVER: &str =
    include_str!("../../runtime/go/v2/server.go");
const RT_V2_ROUTER_GLUE: &str =
    include_str!("../../runtime/go/v2/router_glue.go");

/// Append go2 transpiled runtime files to `files` when
/// `ROUNDHOUSE_GO_V2=1`. No-op otherwise — the default emit pipeline
/// (legacy go) ships unchanged.
pub fn overlay_v2(files: &mut Vec<EmittedFile>, app: &App) {
    if std::env::var("ROUNDHOUSE_GO_V2").as_deref() != Ok("1") {
        return;
    }
    files.extend(emit_overlay_files(app));
}

/// Produce the v2 overlay files unconditionally — for tests and
/// other callers that want the overlay output without setting an env
/// var. Returns the same files `overlay_v2` would append, in
/// emission order.
pub fn emit_overlay_files(app: &App) -> Vec<EmittedFile> {
    let mut out = Vec::new();

    // Hand-written runtime — copied verbatim under `app/v2/`.
    // Emitted FIRST so the transpiled framework runtime files can
    // assume their types resolve at parse time.
    // Always-on overlay shims — stdlib-only imports, ship unconditionally
    // with `ROUNDHOUSE_GO_V2=1`. Model-only shims (db, broadcasts) ship
    // below behind the models env var so the default toolchain test
    // (which doesn't tidy go.mod for app-only sqlite dependencies)
    // stays clean.
    for (name, src) in [
        ("adapter_interface.go", RT_V2_ADAPTER_INTERFACE),
        ("param_value.go", RT_V2_PARAM_VALUE),
    ] {
        out.push(EmittedFile {
            path: PathBuf::from(format!("app/v2/{name}")),
            content: src.to_string(),
        });
    }

    let units = match crate::runtime_loader::go_units(|_ns, classes| {
        lower::lower_for_go(classes)
    }) {
        Ok(u) => u,
        Err(e) => {
            // Transpile failure surfaces as a sentinel file rather
            // than panicking — `go build` picks it up and the cargo
            // test that exercises overlay sees a non-empty result.
            out.push(EmittedFile {
                path: PathBuf::from("app/v2/transpile_error.txt"),
                content: format!("go2 transpile failed: {e}\n"),
            });
            return out;
        }
    };

    for unit in &units {
        // The runtime_loader produces paths shaped like `app/X.go`
        // from GO_RUNTIME; relocate everything under `app/v2/` and
        // re-anchor the package to `v2` so this overlay can never
        // collide with legacy runtime types of the same name
        // (Inflector, JsonBuilder, Router, ...).
        let file_name = unit
            .out_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unit.go".to_string());
        let out_path = PathBuf::from(format!("app/v2/{file_name}"));
        let content = rewrite_package_to_v2(&unit.content);
        out.push(EmittedFile {
            path: out_path,
            content,
        });
    }

    // Phase 3: app models → `app/v2/<snake>.go`. Inventory-mode for
    // now — env-gated so the toolchain test (which runs `go vet` over
    // the whole v2/ overlay) stays clean while the model-emit gaps
    // close one at a time. `ROUNDHOUSE_GO_V2_MODELS=1` opts in; the
    // default emit (and the existing inflector_v2_compiles_and_runs
    // smoke test) ships unchanged. Mirrors rust2's Phase 5 pattern.
    if std::env::var("ROUNDHOUSE_GO_V2_MODELS").as_deref() == Ok("1")
        && !app.models.is_empty()
    {
        // Model-only runtime shims. Db_* bridges database/sql for the
        // per-model adapter_emit lowerer's `Db.prepare(sql)` /
        // `Db.step?(stmt)` / `Db.column_*` calls; Broadcasts_* captures
        // the broadcasts_to-emitted `Broadcasts.prepend(...)` log.
        // Both have no useful Ruby implementation to transpile —
        // mirrors `runtime/rust/db.rs` + `runtime/rust/broadcasts.rs`
        // hand-written rationale. Gated here (not at the always-on
        // overlay above) because db.go pulls modernc.org/sqlite into
        // go.mod and the default toolchain test won't tidy.
        for (name, src) in [
            ("db.go", RT_V2_DB),
            ("broadcasts.go", RT_V2_BROADCASTS),
            // Phase 4 minimum wedge — server boot + empty-mux router
            // glue so the emitted main.go compiles. Per-route binding
            // lands in the next wedge.
            ("server.go", RT_V2_SERVER),
            ("router_glue.go", RT_V2_ROUTER_GLUE),
        ] {
            out.push(EmittedFile {
                path: PathBuf::from(format!("app/v2/{name}")),
                content: src.to_string(),
            });
        }

        // schema_sql.go — DDL constant the boot path passes to
        // OpenProductionDB. Reuses the target-neutral schema renderer
        // (same source the legacy go target consumes for its own
        // app/schema_sql.go).
        let ddl = crate::emit::shared::schema_sql::render_schema_sql(&app.schema);
        out.push(EmittedFile {
            path: PathBuf::from("app/v2/schema_sql.go"),
            content: format!(
                "// Generated by Roundhouse (go2).\npackage v2\n\nconst CreateTables = `\n{ddl}`\n",
            ),
        });

        // main.go — package-main binary entry, sibling to the legacy
        // `<root>/main.go`. Placed at `cmd/v2/main.go` so `go build
        // ./cmd/v2/` builds the v2 binary without colliding with the
        // legacy main.go's package main. Mirrors rust2's `src/main.rs`
        // template emit (wedge 2b).
        out.push(EmittedFile {
            path: PathBuf::from("cmd/v2/main.go"),
            content: "// Generated by Roundhouse (go2).\npackage main\n\n\
import (\n\
\t\"os\"\n\n\
\tv2 \"app/app/v2\"\n\
)\n\n\
func main() {\n\
\tv2.Server_start(v2.Router(), v2.StartOptions{\n\
\t\tDBPath:    os.Getenv(\"DATABASE_PATH\"),\n\
\t\tPort:      os.Getenv(\"PORT\"),\n\
\t\tSchemaSQL: v2.CreateTables,\n\
\t})\n\
}\n".to_string(),
        });

        // Controllers' permit(...) calls feed the params-spec map; models
        // use it to synthesize a typed `from_params(p: <Resource>Params)`
        // factory whose body assigns each permitted field through the
        // column setter. Controller `Model.new(<resource>_params)` call
        // sites are rewritten to `Model.from_params(...)` by the
        // controller lowerer's `rewrite_model_new_to_from_params`; without
        // the factory those calls would land on an undefined symbol.
        // Mirrors rust2.rs / typescript.rs / crystal.rs.
        let params_specs_full =
            crate::lower::controller_to_library::params::collect_specs(&app.controllers);
        let params_specs_simple: std::collections::BTreeMap<
            crate::ident::Symbol,
            Vec<crate::ident::Symbol>,
        > = params_specs_full
            .iter()
            .map(|(r, s)| (r.clone(), s.fields.clone()))
            .collect();

        // `lower_models_with_registry_and_params` returns both the
        // lowered model LCs and the FULL class registry — the registry
        // carries the AR baseline class methods (`find`, `all`, `new`,
        // `count`, etc.) that the model lowerer adds via `insert_default`
        // but never lifts into `lc.methods`. Without those entries the
        // controller body-typer sees `Article.find(...)` as TyVar and
        // the `@article = Article.find(...)` Assign's value.ty stays
        // None, so `collect_fields` lowers the struct field to
        // `interface{}` instead of `*Article`. Mirrors the rust2 wiring
        // (src/emit/rust2.rs:402) — the previous `class_info_from_library_class`
        // shape only carried synthesized table methods and was lossy.
        let (model_lcs, model_registry) =
            crate::lower::model_to_library::lower_models_with_registry_and_params(
                &app.models,
                &app.schema,
                vec![],
                &params_specs_simple,
            );
        let lowered_models = lower::lower_for_go(model_lcs);

        // Controllers are lowered next so we can build the variadic-
        // ctor registry across the full set (runtime + models +
        // controllers) before any LC's emit fires. Synthesized
        // embedded-parent ctors need to match the parent's ctor
        // variadicity — see library::emit_default_embedded_constructor.
        let model_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
            model_registry.into_iter().collect();
        let controller_lcs = crate::lower::controller_to_library::lower_controllers_with_arel_and_views(
            &app.controllers,
            model_extras.clone(),
            Some(&app.schema),
            &app.views,
        );
        let lowered_controllers = lower::lower_for_go(controller_lcs);

        // RouteHelpers module — `lower_routes_to_library_functions` produces
        // one LibraryFunction per named route; bundling them into a
        // module-flavored LibraryClass lets the same library-emit pipeline
        // produce bare functions `RouteHelpers_<name>_path(...)` that
        // controller `RouteHelpers.<name>_path(...)` call sites already
        // resolve through the generic Const-method dispatch in expr.rs.
        let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
        let lowered_route_helpers: Vec<crate::dialect::LibraryClass> =
            if route_helper_funcs.is_empty() {
                Vec::new()
            } else {
                lower::lower_for_go(vec![module_funcs_to_library_class(
                    "RouteHelpers",
                    &route_helper_funcs,
                )])
            };

        // Importmap module — lowered just like RouteHelpers. Layout
        // views call `Importmap.pins()` and `Importmap.entry()` to
        // emit `<script type="importmap">` / `<script>` tags.
        let importmap_funcs = crate::lower::lower_importmap_to_library_functions(app);
        let lowered_importmap: Vec<crate::dialect::LibraryClass> =
            if importmap_funcs.is_empty() {
                Vec::new()
            } else {
                lower::lower_for_go(vec![module_funcs_to_library_class(
                    "Importmap",
                    &importmap_funcs,
                )])
            };

        // Views — real emit (Phase 4 wedge 16). Mirrors rust2 pattern
        // (src/emit/rust2.rs:435-487): lower HTML + JBuilder variants
        // against the model_registry + route_helpers extras, merge
        // both formats under a single `Views::<Resource>` LibraryClass
        // (HTML produces `index`/`show`/`article`; JBuilder produces
        // `index_json`/`show_json`/`article_json` — controllers
        // dispatch by request_format and call into the same module).
        let view_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> = {
            let mut ex = model_extras.clone();
            ex.extend(crate::lower::library_extras::extras_from_funcs(
                &route_helper_funcs,
            ));
            ex
        };
        let lowered_views: Vec<crate::dialect::LibraryClass> = if !app.views.is_empty() {
            let mut raw_lcs = crate::lower::view_to_library::lower_views_to_library_classes(
                &app.views,
                app,
                view_extras.clone(),
            );
            raw_lcs.extend(crate::lower::lower_jbuilder_to_library_classes(
                &app.views,
                app,
                view_extras,
            ));
            // Merge HTML + JSON variants by struct name (rust2:462-479).
            let mut merged: std::collections::BTreeMap<
                String,
                crate::dialect::LibraryClass,
            > = std::collections::BTreeMap::new();
            for lc in raw_lcs {
                let raw = lc.name.0.as_str();
                let struct_name = raw.rsplit("::").next().unwrap_or(raw).to_string();
                merged
                    .entry(struct_name)
                    .and_modify(|acc: &mut crate::dialect::LibraryClass| {
                        let seen: std::collections::BTreeSet<String> = acc
                            .methods
                            .iter()
                            .map(|m| m.name.as_str().to_string())
                            .collect();
                        for m in lc.methods.clone() {
                            if !seen.contains(m.name.as_str()) {
                                acc.methods.push(m);
                            }
                        }
                    })
                    .or_insert(lc);
            }
            lower::lower_for_go(merged.into_values().collect())
        } else {
            Vec::new()
        };

        // Build variadic-ctor registry. Transitive closure: a class is
        // variadic if its `initialize` has trailing-optional/variadic
        // params, OR (no own initialize) its parent is variadic.
        let mut variadic_ctors: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        loop {
            let before = variadic_ctors.len();
            for lc in units.iter().flat_map(|u| u.classes.iter())
                .chain(lowered_models.iter())
                .chain(lowered_controllers.iter())
                .chain(lowered_route_helpers.iter())
                .chain(lowered_importmap.iter())
                .chain(lowered_views.iter())
            {
                let sanitized = lc.name.0.as_str().replace("::", "");
                if variadic_ctors.contains(&sanitized) {
                    continue;
                }
                let has_init = lc.methods.iter().any(|m| {
                    matches!(m.receiver, crate::dialect::MethodReceiver::Instance)
                        && m.name.as_str() == "initialize"
                });
                let init_is_variadic = lc.methods.iter().find(|m| {
                    matches!(m.receiver, crate::dialect::MethodReceiver::Instance)
                        && m.name.as_str() == "initialize"
                }).map(|m| {
                    // render_constructor_params lifts trailing-defaulted
                    // params into a Go variadic — match the same predicate
                    // so the registry stays consistent.
                    m.params.iter().any(|p| p.default.is_some())
                }).unwrap_or(false);
                if has_init {
                    if init_is_variadic {
                        variadic_ctors.insert(sanitized);
                    }
                } else if let Some(parent) = lc.parent.as_ref() {
                    let parent_san = parent.0.as_str().replace("::", "");
                    if variadic_ctors.contains(&parent_san) {
                        variadic_ctors.insert(sanitized);
                    }
                }
            }
            if variadic_ctors.len() == before {
                break;
            }
        }

        for lc in &lowered_models {
            let class_text = match library::emit_library_class_with_registry(lc, &variadic_ctors) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            // Wrap with `package app` so rewrite_package_to_v2 swaps
            // to `package v2` and injects per-file stdlib imports.
            let raw = format!("// Generated from app/models/{stem}.rb at app emit time.\n// Do not edit by hand — edit the source `.rb` and re-run emit.\n\npackage app\n\n{class_text}",
                stem = crate::naming::snake_case(lc.name.0.as_str()),
            );
            let content = rewrite_package_to_v2(&raw);
            let stem = crate::naming::snake_case(lc.name.0.as_str());
            out.push(EmittedFile {
                path: PathBuf::from(format!("app/v2/{stem}.go")),
                content,
            });
        }

        for lc in &lowered_controllers {
            let class_text = match library::emit_library_class_with_registry(lc, &variadic_ctors) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            let stem = crate::naming::snake_case(lc.name.0.as_str());
            let raw = format!(
                "// Generated from app/controllers/{stem}.rb at app emit time.\n// Do not edit by hand — edit the source `.rb` and re-run emit.\n\npackage app\n\n{class_text}",
            );
            let content = rewrite_package_to_v2(&raw);
            out.push(EmittedFile {
                path: PathBuf::from(format!("app/v2/{stem}.go")),
                content,
            });
        }

        for lc in &lowered_route_helpers {
            let class_text = match library::emit_library_class_with_registry(lc, &variadic_ctors) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            let raw = format!(
                "// Generated by Roundhouse from config/routes.rb.\n// Do not edit by hand — edit the source `routes.rb` and re-run emit.\n\npackage app\n\n{class_text}",
            );
            let content = rewrite_package_to_v2(&raw);
            out.push(EmittedFile {
                path: PathBuf::from("app/v2/route_helpers.go".to_string()),
                content,
            });
        }

        for lc in &lowered_importmap {
            let class_text = match library::emit_library_class_with_registry(lc, &variadic_ctors) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            let raw = format!(
                "// Generated by Roundhouse from config/importmap.rb.\n// Do not edit by hand — edit the source `importmap.rb` and re-run emit.\n\npackage app\n\n{class_text}",
            );
            let content = rewrite_package_to_v2(&raw);
            out.push(EmittedFile {
                path: PathBuf::from("app/v2/importmap.go".to_string()),
                content,
            });
        }

        // Phase 4 wedge 16 — real view emit. Replaces the variadic
        // `_args ...interface{}) string` stubs with concrete typed
        // bodies emitted from the merged HTML+JBuilder LibraryClasses
        // (lowered above). Mirrors rust2's view-emit pattern
        // (src/emit/rust2.rs:743-).
        for lc in &lowered_views {
            let raw_name = lc.name.0.as_str();
            let struct_name = raw_name.rsplit("::").next().unwrap_or(raw_name).to_string();
            let stem = crate::naming::snake_case(&struct_name);
            let class_text = match library::emit_library_class_with_registry(lc, &variadic_ctors) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            let raw = format!(
                "// Generated from app/views/{stem}/ at app emit time.\n// Do not edit by hand — edit the source view templates and re-run emit.\n\npackage app\n\n{class_text}",
            );
            let content = rewrite_package_to_v2(&raw);
            out.push(EmittedFile {
                path: PathBuf::from(format!("app/v2/views_{stem}.go")),
                content,
            });
        }

        // Phase 4 wedge 15 — route table + dispatcher. Together with
        // the updated `runtime/go/v2/router_glue.go`, these turn the
        // boot-time empty mux into a real handler that matches
        // requests via the transpiled `ActionDispatchRouter_match`
        // table and dispatches into per-controller `ProcessAction`
        // entry points. GET routes serve real responses (view bodies
        // are still stubbed → empty strings; the next wedge wires
        // real views). POST/PATCH/DELETE pass through but with no
        // form-body parsing yet, so writes land with empty params.
        // Mirrors rust2 wedges 2c.1 + 2c.2 collapsed: Go's plain
        // struct-field response state (Body/Status/ContentType/
        // Location promoted from the embedded *ActionControllerBase)
        // sidesteps rust2's thread_local + axum-extractor dance.
        out.push(emit_routes_table_file(app));
        out.push(emit_dispatch_file(app));
    }

    out
}

/// Emit `app/v2/routes_table.go` — a single `RoutesTable` slice of
/// `*ActionDispatchRouterRoute` entries built from `flatten_routes(app)`.
/// `runtime/go/v2/router_glue.go` reads this table on every request and
/// calls the transpiled `ActionDispatchRouter_match` to locate the
/// `(controller, action, path_params)` triple to dispatch.
fn emit_routes_table_file(app: &App) -> EmittedFile {
    use crate::dialect::HttpMethod;
    let flat = crate::lower::flatten_routes(app);
    let mut s = String::new();
    s.push_str("// Generated by Roundhouse (go2 wedge 15).\n");
    s.push_str("// Do not edit by hand — edit `config/routes.rb` and re-run emit.\n\n");
    s.push_str("package v2\n\n");
    s.push_str("// RoutesTable enumerates every Rails-style route the app exposes.\n");
    s.push_str("// router_glue.go scans this table on every request via\n");
    s.push_str("// ActionDispatchRouter_match (transpiled from runtime/ruby/\n");
    s.push_str("// action_dispatch/router.rb).\n");
    s.push_str("var RoutesTable = []*ActionDispatchRouterRoute{\n");
    for r in &flat {
        let verb = match r.method {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Any => "GET",
        };
        s.push_str(&format!(
            "\tNewActionDispatchRouterRoute({verb:?}, {path:?}, {ctrl:?}, {action:?}),\n",
            path = r.path,
            ctrl = r.controller.0.as_str(),
            action = r.action.as_str(),
        ));
    }
    s.push_str("}\n");
    EmittedFile {
        path: PathBuf::from("app/v2/routes_table.go"),
        content: s,
    }
}

/// Emit `app/v2/dispatch.go` — `Dispatch(controller, action, path_params,
/// r)` constructs the named controller, threads request metadata + path
/// params into it, invokes `ProcessAction(action)`, and returns the
/// controller's response state (Body/Status/ContentType/Location, all
/// reachable via Go field promotion from the embedded
/// *ActionControllerBase). One switch arm per concrete controller in
/// the app. Unknown controller names return a 404.
fn emit_dispatch_file(app: &App) -> EmittedFile {
    let mut s = String::new();
    s.push_str("// Generated by Roundhouse (go2 wedge 15).\n");
    s.push_str("// Do not edit by hand — controller list is derived from app/controllers/.\n\n");
    s.push_str("package v2\n\n");
    s.push_str("import (\n\t\"net/http\"\n\t\"strings\"\n)\n\n");
    s.push_str("// Dispatch builds a controller for (controller, action), threads\n");
    s.push_str("// request metadata into it, runs the action, and returns the\n");
    s.push_str("// captured response state. POST/PATCH/PUT bodies are not yet\n");
    s.push_str("// form-parsed; those wedges follow.\n");
    s.push_str(
        "func Dispatch(controller string, action string, pathParams map[string]string, r *http.Request) (body string, status int64, contentType string, location string) {\n",
    );
    s.push_str("\tparams := map[string]RoundhouseParamValue{}\n");
    s.push_str("\tfor k, v := range pathParams {\n");
    s.push_str("\t\tparams[k] = v\n");
    s.push_str("\t}\n");
    s.push_str("\trequestFormat := \"html\"\n");
    s.push_str("\tif strings.HasSuffix(r.URL.Path, \".json\") {\n");
    s.push_str("\t\trequestFormat = \"json\"\n");
    s.push_str("\t}\n\n");
    s.push_str("\tswitch controller {\n");
    for ctrl in &app.controllers {
        let raw = ctrl.name.0.as_str();
        let struct_name = raw.rsplit("::").next().unwrap_or(raw);
        s.push_str(&format!("\tcase {raw:?}:\n"));
        s.push_str(&format!("\t\tc := New{struct_name}()\n"));
        s.push_str("\t\tc.Params = params\n");
        s.push_str("\t\tc.RequestMethod = r.Method\n");
        s.push_str("\t\tc.RequestPath = r.URL.Path\n");
        s.push_str("\t\tc.RequestFormat = requestFormat\n");
        s.push_str("\t\tc.ProcessAction(action)\n");
        s.push_str("\t\treturn c.Body, c.Status, c.ContentType, c.Location\n");
    }
    s.push_str("\t}\n");
    s.push_str("\treturn \"\", 404, \"\", \"\"\n");
    s.push_str("}\n");
    EmittedFile {
        path: PathBuf::from("app/v2/dispatch.go"),
        content: s,
    }
}

/// Replace the leading `package app` declaration with `package v2`
/// and inject `import` declarations for stdlib packages the body
/// references (`cmp`, `fmt`, `regexp`, `slices`, `strings`, `time`).
/// Go is strict about unused imports so detection is by substring
/// presence, not always-on.
/// `Views::Articles` → `ViewsArticles`. Same logic as
/// `library.rs::sanitize_type_name`, repeated here so the stub-emit
/// path doesn't need to reach into library internals for a one-liner.
fn sanitize_module_name(name: &str) -> String {
    name.replace("::", "")
}

/// Bundle a flat list of `LibraryFunction`s (e.g. route helpers,
/// importmap entries) into a module-flavored `LibraryClass` so the
/// shared `library::emit_library_class_with_registry` pipeline can
/// produce bare `<Module>_<name>(...)` functions. Mirrors the same-
/// named helper in `src/emit/rust2.rs`; duplicated here to keep go2
/// independent of rust2 internals.
fn module_funcs_to_library_class(
    name: &str,
    funcs: &[crate::dialect::LibraryFunction],
) -> crate::dialect::LibraryClass {
    use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
    use crate::ident::ClassId;
    let methods: Vec<MethodDef> = funcs
        .iter()
        .map(|f| MethodDef {
            name: f.name.clone(),
            receiver: MethodReceiver::Class,
            params: f.params.clone(),
            body: f.body.clone(),
            signature: f.signature.clone(),
            effects: f.effects.clone(),
            enclosing_class: Some(crate::ident::Symbol::from(name)),
            kind: AccessorKind::Method,
            is_async: f.is_async,
            mutates_self: false,
        })
        .collect();
    LibraryClass {
        name: ClassId(crate::ident::Symbol::from(name)),
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods,
        origin: None,
    }
}

fn rewrite_package_to_v2(content: &str) -> String {
    let imports = needed_imports(content);
    let mut out = String::with_capacity(content.len() + 64);
    let mut saw_pkg = false;
    let mut imports_emitted = false;
    for line in content.lines() {
        if !saw_pkg && line.starts_with("package ") {
            out.push_str("package v2\n");
            saw_pkg = true;
        } else {
            // First non-package, non-comment line is where the imports
            // go. Pre-pending here keeps them above transpiled headers.
            if saw_pkg && !imports_emitted && !imports.is_empty()
                && !line.starts_with("//")
                && !line.trim().is_empty()
            {
                emit_imports(&mut out, &imports);
                imports_emitted = true;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    // Fallthrough: file had only comments + package line. Still emit
    // imports if needed (the body might be empty, but that's harmless).
    if saw_pkg && !imports_emitted && !imports.is_empty() {
        emit_imports(&mut out, &imports);
    }
    if !saw_pkg {
        let mut prefixed = String::from("package v2\n\n");
        if !imports.is_empty() {
            emit_imports(&mut prefixed, &imports);
        }
        prefixed.push_str(&out);
        return prefixed;
    }
    out
}

fn needed_imports(content: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    // Each entry is `(probes[], package)`. A package gets imported
    // when ANY of its probes is present in `content`. Most stdlib
    // packages have a unique-enough top-level prefix (`fmt.`,
    // `strings.`, …) that the bare-period probe is safe; `time.` is
    // an exception because the file header comment "at app emit time."
    // false-matches, so its probes specifically target the call sites
    // the peepholes emit (`time.Now(`, `time.RFC3339`). Add probes
    // here when a new `time.X` emit lands.
    let entries: &[(&[&str], &str)] = &[
        (&["base64."], "encoding/base64"),
        (&["cmp."], "cmp"),
        (&["fmt."], "fmt"),
        (&["json."], "encoding/json"),
        (&["regexp."], "regexp"),
        (&["slices."], "slices"),
        (&["strconv."], "strconv"),
        (&["strings."], "strings"),
        (&["time.Now(", "time.RFC3339"], "time"),
    ];
    for (probes, name) in entries {
        if probes.iter().any(|p| content.contains(p)) {
            out.push(*name);
        }
    }
    out
}

fn emit_imports(out: &mut String, imports: &[&str]) {
    if imports.len() == 1 {
        out.push_str(&format!("import {:?}\n\n", imports[0]));
    } else {
        out.push_str("import (\n");
        for name in imports {
            out.push_str(&format!("\t{name:?}\n"));
        }
        out.push_str(")\n\n");
    }
}

// Re-export the library emit functions used by `runtime_loader::GO_TARGET`.
// `emit_library_class` is `pub` (not `pub(crate)`) so integration
// tests in `tests/` can synthesize a `LibraryClass` and assert the
// emitted shape without going through the GO_RUNTIME wiring — that
// keeps shape coverage decoupled from "is this file currently in
// the v2/ overlay" gating.
pub use library::{emit_library_class, emit_library_class_with_registry};
pub(crate) use library::{emit_module, format_constant, format_module_ivar};
