//! Go target — `go2` emit.
//!
//! Built via the `rust2` migration pattern. As of the go→go2
//! switchover (Phase 6 step 3) this is the sole Go emit path: the
//! legacy per-target tree was deleted and `src/emit/go.rs` shrank to
//! a `pub fn emit` shim that delegates here (plus a self-contained
//! `emit_method` runtime-extraction helper). The post-cleanup move
//! also pulled the remaining shared helpers (`shared` naming +
//! `emit_literal`) and the test emitters (`fixture`, `spec`,
//! `controller_test`) into this module, so go2 no longer reaches back
//! into a `go/` directory.
//!
//! Output is the lowered-IR + transpiled `runtime/ruby/` framework
//! set, emitted under `app/v2/` (its own Go package) alongside the
//! hand-written primitive runtime copied verbatim from
//! `runtime/go/v2/` (cable, server, db, …).

use super::EmittedFile;
use crate::App;

mod controller_test;
mod expr;
mod fixture;
mod imports;
mod library;
pub mod lower;
mod paths;
mod shared;
mod spec;
pub(crate) mod ty;

use imports::FileImports;
use paths::{output_path, OutputKind};

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
const RT_V2_ERRORS: &str =
    include_str!("../../runtime/go/v2/errors.go");
const RT_V2_MODELER: &str =
    include_str!("../../runtime/go/v2/modeler.go");
const RT_V2_DB: &str =
    include_str!("../../runtime/go/v2/db.go");
const RT_V2_BROADCASTS: &str =
    include_str!("../../runtime/go/v2/broadcasts.go");
const RT_V2_CABLE: &str =
    include_str!("../../runtime/go/v2/cable.go");
const RT_V2_SERVER: &str =
    include_str!("../../runtime/go/v2/server.go");
const RT_V2_ROUTER_GLUE: &str =
    include_str!("../../runtime/go/v2/router_glue.go");
const RT_V2_FORM_PARAMS: &str =
    include_str!("../../runtime/go/v2/form_params.go");
const RT_V2_SLOTS: &str =
    include_str!("../../runtime/go/v2/slots.go");

/// Append go2 transpiled runtime files to `files`. Phase 6 step 2
/// (2026-05-24) flipped the env-var gate to unconditional: the v2
/// overlay is now part of every `bin/rh transpile go` output. The
/// legacy emit still produces `app/*.go` alongside `app/v2/*.go`
/// for the moment; Phase 6 step 3 will delete the legacy half.
pub fn overlay_v2(files: &mut Vec<EmittedFile>, app: &App) {
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
        ("errors.go", RT_V2_ERRORS),
        ("modeler.go", RT_V2_MODELER),
        // Per-goroutine slot store for content_for/yield — owns the
        // six ActionViewViewHelpers_<slot-method> package-level
        // functions that the auto-emit of view_helpers.rb would
        // otherwise produce against a racy package-level map. The
        // suppression of the auto-emit lives in `format_module_ivar`
        // (drops `@slots = {}`) and in `go_units`'s transform
        // callback below (drops the six slot methods from the
        // ActionView::ViewHelpers LibraryClass before
        // emit_library_class walks it).
        ("slots.go", RT_V2_SLOTS),
    ] {
        out.push(EmittedFile {
            path: output_path(OutputKind::HandWrittenRuntime { name }).path,
            content: src.to_string(),
        });
    }

    let units = match crate::runtime_loader::go_units(|_ns, classes| {
        // ActionView::ViewHelpers's six slot methods (reset_slots!,
        // content_for_set/get, get_slot, get_yield, set_yield) are
        // owned by the hand-written runtime/go/v2/slots.go shim —
        // per-goroutine storage replaces the package-level map +
        // RWMutex from commit 1f2a984. Drop them from the LibraryClass
        // before lower_for_go / emit_library_class walks it so the
        // auto-emit doesn't produce colliding `func
        // ActionViewViewHelpers_<slot-method>` definitions.
        let classes = classes
            .into_iter()
            .map(|mut c| {
                if c.name.0.as_str() == "ActionView::ViewHelpers" {
                    c.methods.retain(|m| {
                        !matches!(
                            m.name.as_str(),
                            "reset_slots!"
                                | "content_for_set"
                                | "content_for_get"
                                | "get_slot"
                                | "get_yield"
                                | "set_yield"
                        )
                    });
                }
                c
            })
            .collect();
        lower::lower_for_go(classes)
    }) {
        Ok(u) => u,
        Err(e) => {
            // Transpile failure surfaces as a sentinel file rather
            // than panicking — `go build` picks it up and the cargo
            // test that exercises overlay sees a non-empty result.
            out.push(EmittedFile {
                path: output_path(OutputKind::TranspileError).path,
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
        let dest = output_path(OutputKind::TranspiledRuntime {
            file_name: &file_name,
        });
        let content = rewrite_package(&unit.content, dest.package, &FileImports::new());
        out.push(EmittedFile {
            path: dest.path,
            content,
        });
    }

    // Phase 6 step 2 (2026-05-24): the ROUNDHOUSE_GO_V2_MODELS env
    // gate is gone — model + controller + view + test emit now ship
    // unconditionally whenever the app has models. Pre-flip, this
    // block was opt-in to let inflector_v2_compiles_and_runs (the
    // base toolchain smoke test) stay clean during the model emit
    // build-out; that smoke now passes alongside the full v2 path.
    if !app.models.is_empty() {
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
            // Action Cable WebSocket endpoint + Turbo Streams fan-out.
            // Gated alongside broadcasts.go (which calls into it) and
            // db.go because it pulls github.com/coder/websocket into
            // go.mod, which the default toolchain test won't tidy.
            ("cable.go", RT_V2_CABLE),
            // Phase 4 minimum wedge — server boot + empty-mux router
            // glue so the emitted main.go compiles. Per-route binding
            // lands in the next wedge.
            ("server.go", RT_V2_SERVER),
            ("router_glue.go", RT_V2_ROUTER_GLUE),
            // ParseFormParams — the emitted dispatch.go calls this on
            // every request to parse x-www-form-urlencoded bodies into
            // nested RoundhouseParamValue maps (Rails-style bracket
            // notation, e.g. `article[title]=Hi`).
            ("form_params.go", RT_V2_FORM_PARAMS),
        ] {
            out.push(EmittedFile {
                path: output_path(OutputKind::HandWrittenRuntime { name }).path,
                content: src.to_string(),
            });
        }

        // schema_sql.go — DDL constant the boot path passes to
        // OpenProductionDB. Reuses the target-neutral schema renderer
        // (same source the legacy go target consumes for its own
        // app/schema_sql.go).
        let ddl = crate::emit::shared::schema_sql::render_schema_sql(&app.schema);
        out.push(EmittedFile {
            path: output_path(OutputKind::SchemaSql).path,
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
            path: output_path(OutputKind::Main).path,
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
        let assocs = crate::lower::model_associations::compute_association_graph(app);
        let controller_lcs = crate::lower::controller_to_library::lower_controllers_with_arel_views_and_assocs(
            &app.controllers,
            model_extras.clone(),
            Some(&app.schema),
            &app.views,
            &assocs,
        );
        // Pass the lowered models as the callee-registry extras so
        // cross-class calls like `Post.find(params[:id])` in a
        // controller body see Post's Int param signature and get a
        // Cast inserted around the Str-flowing-into-Int arg. Without
        // extras, the controller-only ty_coerce_insertion pass can't
        // resolve Post_find and the coercion never fires.
        let lowered_controllers = lower::lower_for_go_with_extras(controller_lcs, &lowered_models);

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

        // Q1 — build the ar_chain registry: every class whose
        // embedding chain reaches `ActiveRecord::Base`. Constructor
        // synthesis wires `self.Self = self` for these so polymorphic
        // dispatch (Base method calls `b.Self.SchemaColumns()`)
        // lands on the outer subclass. Computed via the same
        // transitive-closure shape as variadic_ctors: start with the
        // root ("ActiveRecord::Base" itself, stored under its
        // sanitized form "ActiveRecordBase"), then walk every LC
        // adding classes whose parent is already in the set. Fixed
        // point.
        let mut ar_chain: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        ar_chain.insert("ActiveRecordBase".to_string());
        loop {
            let before = ar_chain.len();
            for lc in units.iter().flat_map(|u| u.classes.iter())
                .chain(lowered_models.iter())
                .chain(lowered_controllers.iter())
                .chain(lowered_route_helpers.iter())
                .chain(lowered_importmap.iter())
                .chain(lowered_views.iter())
            {
                let sanitized = lc.name.0.as_str().replace("::", "");
                if ar_chain.contains(&sanitized) {
                    continue;
                }
                if let Some(parent) = lc.parent.as_ref() {
                    let parent_san = parent.0.as_str().replace("::", "");
                    if ar_chain.contains(&parent_san) {
                        ar_chain.insert(sanitized);
                    }
                }
            }
            if ar_chain.len() == before {
                break;
            }
        }

        for lc in &lowered_models {
            let class_text = match library::emit_library_class_with_registry(lc, &variadic_ctors, &ar_chain) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            // Wrap with `package app` so rewrite_package swaps to the
            // model's target package (`v2` today; per Phase 4 of #19
            // will become `models` for the `internal/models/` cutover)
            // and injects per-file stdlib imports.
            let raw = format!("// Generated from app/models/{stem}.rb at app emit time.\n// Do not edit by hand — edit the source `.rb` and re-run emit.\n\npackage app\n\n{class_text}",
                stem = crate::naming::snake_case(lc.name.0.as_str()),
            );
            let dest = output_path(OutputKind::Model {
                lc_name: lc.name.0.as_str(),
            });
            let content = rewrite_package(&raw, dest.package, &FileImports::new());
            out.push(EmittedFile {
                path: dest.path,
                content,
            });
        }

        for lc in &lowered_controllers {
            let class_text = match library::emit_library_class_with_registry(lc, &variadic_ctors, &ar_chain) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            let stem = crate::naming::snake_case(lc.name.0.as_str());
            let raw = format!(
                "// Generated from app/controllers/{stem}.rb at app emit time.\n// Do not edit by hand — edit the source `.rb` and re-run emit.\n\npackage app\n\n{class_text}",
            );
            let dest = output_path(OutputKind::Controller {
                lc_name: lc.name.0.as_str(),
            });
            let content = rewrite_package(&raw, dest.package, &FileImports::new());
            out.push(EmittedFile {
                path: dest.path,
                content,
            });
        }

        // Synthesize stub ApplicationRecord / ApplicationController
        // when the source app doesn't ship `app/models/application_record.rb`
        // / `app/controllers/application_controller.rb` (tiny-blog
        // fixture). Concrete model/controller emit references these
        // names via the parent embedding chain (Article embeds
        // *ApplicationRecord → *ActiveRecordBase; PostsController
        // embeds *ApplicationController → *ActionControllerBase).
        // Without the file, `go vet` flags the embed as undefined.
        // The stubs are pass-through: ApplicationRecord embeds
        // *ActiveRecordBase + ctor delegates; ApplicationController
        // does the same for AC::Base. Mirrors Rails' implicit
        // ApplicationRecord/ApplicationController auto-generation.
        let has_application_record = lowered_models
            .iter()
            .any(|lc| lc.name.0.as_str() == "ApplicationRecord");
        if !has_application_record && !lowered_models.is_empty() {
            out.push(EmittedFile {
                path: output_path(OutputKind::SynthApplicationRecord).path,
                content: "// Synthesized by Roundhouse (go2) — the source\n\
                          // app doesn't ship app/models/application_record.rb,\n\
                          // so emit a pass-through stub for the AR-chain\n\
                          // embedding to resolve.\n\n\
                          package v2\n\n\
                          type ApplicationRecord struct {\n\
                          \t*ActiveRecordBase\n\
                          }\n\n\
                          func NewApplicationRecord(_opts ...map[string]interface{}) *ApplicationRecord {\n\
                          \tself := &ApplicationRecord{ActiveRecordBase: NewActiveRecordBase(_opts...)}\n\
                          \tself.Self = self\n\
                          \treturn self\n\
                          }\n"
                    .to_string(),
            });
        }
        let has_application_controller = lowered_controllers
            .iter()
            .any(|lc| lc.name.0.as_str() == "ApplicationController");
        if !has_application_controller && !lowered_controllers.is_empty() {
            out.push(EmittedFile {
                path: output_path(OutputKind::SynthApplicationController).path,
                content: "// Synthesized by Roundhouse (go2) — the source\n\
                          // app doesn't ship app/controllers/application_controller.rb,\n\
                          // so emit a pass-through stub for the AC-chain\n\
                          // embedding to resolve.\n\n\
                          package v2\n\n\
                          type ApplicationController struct {\n\
                          \t*ActionControllerBase\n\
                          }\n\n\
                          func NewApplicationController() *ApplicationController {\n\
                          \treturn &ApplicationController{ActionControllerBase: NewActionControllerBase()}\n\
                          }\n"
                    .to_string(),
            });
        }

        for lc in &lowered_route_helpers {
            let class_text = match library::emit_library_class_with_registry(lc, &variadic_ctors, &ar_chain) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            let raw = format!(
                "// Generated by Roundhouse from config/routes.rb.\n// Do not edit by hand — edit the source `routes.rb` and re-run emit.\n\npackage app\n\n{class_text}",
            );
            let dest = output_path(OutputKind::RouteHelpers);
            let content = rewrite_package(&raw, dest.package, &FileImports::new());
            out.push(EmittedFile {
                path: dest.path,
                content,
            });
        }

        for lc in &lowered_importmap {
            let class_text = match library::emit_library_class_with_registry(lc, &variadic_ctors, &ar_chain) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            let raw = format!(
                "// Generated by Roundhouse from config/importmap.rb.\n// Do not edit by hand — edit the source `importmap.rb` and re-run emit.\n\npackage app\n\n{class_text}",
            );
            let dest = output_path(OutputKind::Importmap);
            let content = rewrite_package(&raw, dest.package, &FileImports::new());
            out.push(EmittedFile {
                path: dest.path,
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
            let class_text = match library::emit_library_class_with_registry(lc, &variadic_ctors, &ar_chain) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            let raw = format!(
                "// Generated from app/views/{stem}/ at app emit time.\n// Do not edit by hand — edit the source view templates and re-run emit.\n\npackage app\n\n{class_text}",
            );
            let dest = output_path(OutputKind::ViewBundle {
                resource_snake: &stem,
            });
            let content = rewrite_package(&raw, dest.package, &FileImports::new());
            out.push(EmittedFile {
                path: dest.path,
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

        // Phase 6 step 1 — wire legacy test emit through v2 so
        // `go test ./app/v2/` exercises the same fixtures + per-model
        // + per-controller test bodies the legacy emit ships. Each
        // emitted file's `package app` header gets rewritten to
        // `package v2` and the path gets re-anchored under `app/v2/`.
        // A `test_compat.go` shim file declares legacy-shape aliases
        // (`ArticleFind` → `Article_find`, `ArticlesPath` →
        // `RouteHelpers_articles_path`, …) so the legacy-emitted test
        // bodies resolve without per-test re-emit. Mirrors rust2 wedge
        // 2c.3's "AR shim last/reload + route_helpers/fixtures bare-fn
        // compat shims" approach (see project_rust2_wedge_2c3_landed.md).
        out.extend(emit_v2_test_files(app));
    }

    out
}

/// Run the go test emitters (fixture + spec) and re-anchor each
/// emitted file under `app/v2/` with `package v2`. The bodies
/// are produced unchanged; compat shims (`emit_v2_test_compat`) bridge
/// the legacy-shape symbol references to the v2 names.
fn emit_v2_test_files(app: &App) -> Vec<EmittedFile> {
    let mut out = Vec::new();
    let model_names: Vec<String> = app
        .models
        .iter()
        .filter(|m| m.attributes.fields.keys().any(|k| k.as_str() != "id"))
        .map(|m| m.name.0.as_str().to_string())
        .collect();
    if !app.fixtures.is_empty() {
        let f = fixture::emit_go_fixtures(app);
        out.push(rewrite_test_file_to_v2(f, &model_names));
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            let f = spec::emit_go_tests(tm, app);
            out.push(rewrite_test_file_to_v2(f, &model_names));
        }
        // test_support + compat shim — only ship when there are
        // tests to consume them.
        out.push(emit_v2_test_support(app));
        out.push(emit_v2_test_compat(app));
    }
    out
}

/// Rewrite an `app/<x>.go` EmittedFile to `app/v2/<x>.go` with
/// `package v2`. Test files already carry their own `import (...)`
/// block from spec.rs, so this is a simple package-line swap rather
/// than the full `rewrite_package` plumbing.
///
/// Constructor literal rewrite: legacy fixture/spec emit produces
/// `&Article{}` for zero-init then field-assignment. Under v2's
/// embedded-`*Parent` shape this lands an ApplicationRecord nil
/// pointer that panics on the first Save(). Replace each `&<Model>{}`
/// with `New<Model>()` so the embedded chain (ApplicationRecord →
/// ActiveRecordBase → Self) initializes correctly.
fn rewrite_test_file_to_v2(f: EmittedFile, model_names: &[String]) -> EmittedFile {
    let new_path = match f.path.strip_prefix("app/") {
        Ok(suffix) => output_path(OutputKind::TestFile {
            file_name: &suffix.to_string_lossy(),
        })
        .path,
        Err(_) => f.path.clone(),
    };
    let mut content = f.content.replacen("package app", "package v2", 1);
    for model in model_names {
        // `&Article{}` → `NewArticle()`
        let from = format!("&{model}{{}}");
        let to = format!("New{model}()");
        content = content.replace(&from, &to);
    }
    EmittedFile {
        path: new_path,
        content,
    }
}

/// Hand-rolled `test_support.go` for v2. Provides `SetupTestDB`,
/// `TestClient`, and `TestResponse` — the surface the legacy spec.rs
/// emit references. Dispatches through v2's `Dispatch` (so it
/// exercises the same router + controllers production traffic uses).
fn emit_v2_test_support(_app: &App) -> EmittedFile {
    let content = include_str!("../../runtime/go/v2/test_support_test.go");
    EmittedFile {
        // `_test.go` suffix keeps this out of `go build` — the
        // overrides + helpers only exist during `go test` and don't
        // affect the production binary.
        path: output_path(OutputKind::TestFile {
            file_name: "test_support_test.go",
        })
        .path,
        content: content.to_string(),
    }
}

/// Compat shims so the legacy-emitted test bodies resolve under v2.
/// Legacy emits `ArticleFind(id)`, `ArticleCount()`, `ArticlesPath()`
/// etc.; v2 names everything per the module-singleton convention
/// (`Article_find`, `Article_count`, `RouteHelpers_articles_path`).
/// One small file mapping legacy names to v2 names is cheaper than
/// touching every emit site in fixture.rs/spec.rs/controller_test.rs.
fn emit_v2_test_compat(app: &App) -> EmittedFile {
    let mut s = String::new();
    s.push_str("// Generated by Roundhouse (go2 Phase 6 step 1).\n");
    s.push_str("// Compat shims for legacy-shape test references — see\n");
    s.push_str("// src/emit/go2.rs::emit_v2_test_compat for rationale.\n\n");
    s.push_str("package v2\n\n");

    // Per-model adapter wrappers. Tests use `ArticleFind(id)`,
    // `ArticleCount()`, `ArticleLast()`; v2 emits `Article_find`,
    // `Article_count`, `Article_last`. Skip abstract base classes
    // (ApplicationRecord) that have only `id` — matches the legacy
    // emit's persistence-method filter.
    for m in &app.models {
        let has_table = m.attributes.fields.keys().any(|k| k.as_str() != "id");
        if !has_table {
            continue;
        }
        let model = m.name.0.as_str();
        s.push_str(&format!(
            "func {model}Find(id int64) *{model} {{ return {model}_find(id) }}\n"
        ));
        s.push_str(&format!(
            "func {model}Count() int64 {{ return {model}_count() }}\n"
        ));
        s.push_str(&format!(
            "func {model}Last() *{model} {{ return {model}_last() }}\n"
        ));
    }
    s.push('\n');

    // Route helpers — legacy emits `ArticlesPath()`, v2 emits
    // `RouteHelpers_articles_path()`. Walk the flat route list and
    // produce one shim per Rails route_helper name (`as_name`).
    let flat = crate::lower::flatten_routes(app);
    let mut seen = std::collections::HashSet::new();
    for route in &flat {
        let as_name = route.as_name.as_str();
        if as_name.is_empty() {
            continue;
        }
        // Path helper (no _path suffix on as_name; legacy adds Path).
        // `articles` → `ArticlesPath` ; v2's `RouteHelpers_articles_path`.
        let legacy = format!("{}Path", crate::naming::camelize(as_name));
        let v2_name = format!("RouteHelpers_{as_name}_path");
        if !seen.insert(legacy.clone()) {
            continue;
        }
        // Param signature follows the v2 helper's signature; recover
        // by counting path params via the route's dynamic segments.
        let dyn_count = route.path_params.len();
        let (params, args) = match dyn_count {
            0 => (String::new(), String::new()),
            1 => ("id int64".to_string(), "id".to_string()),
            2 => (
                "a int64, b int64".to_string(),
                "a, b".to_string(),
            ),
            _ => continue, // skip unusual arities
        };
        s.push_str(&format!(
            "func {legacy}({params}) string {{ return {v2_name}({args}) }}\n"
        ));
    }
    // Per-AR-subclass Save() override that bootstraps the embedded
    // chain + Self back-pointer. Tests construct via `&Article{}`
    // literal init which leaves ApplicationRecord nil; production
    // callers use `NewArticle()` so this defensive init only fires
    // in test builds (the `_test.go` suffix limits exposure).
    s.push('\n');
    for m in &app.models {
        let has_table = m.attributes.fields.keys().any(|k| k.as_str() != "id");
        if !has_table {
            continue;
        }
        let model = m.name.0.as_str();
        s.push_str(&format!(
            "// Test-only Save() override — initializes the embedded\n\
             // *ApplicationRecord and the Modeler back-pointer when a\n\
             // test uses `&{model}{{...}}` literal init. The Self\n\
             // assignment ALWAYS overwrites (not nil-guarded) because\n\
             // NewApplicationRecord() seeds Self to *ApplicationRecord;\n\
             // we need the outermost *{model} for SchemaColumns +\n\
             // OpGet/OpSet polymorphic dispatch via Modeler.\n\
             func (self *{model}) Save() bool {{\n\
             \tif self.ApplicationRecord == nil {{ self.ApplicationRecord = NewApplicationRecord() }}\n\
             \tself.Self = self\n\
             \treturn self.ActiveRecordBase.Save()\n\
             }}\n\n\
             // Test-only Reload() override. AR::Base's Reload calls\n\
             // `self.AdapterReload()` against *ActiveRecordBase, which\n\
             // is a no-op stub. AdapterReload can't join the Modeler\n\
             // interface because its return type (`*{model}`) varies\n\
             // per subclass. The targeted override dispatches to the\n\
             // subclass's AdapterReload which writes fresh DB row data\n\
             // back into the same struct via field promotion.\n\
             func (self *{model}) Reload() *{model} {{\n\
             \tself.AdapterReload()\n\
             \treturn self\n\
             }}\n\n"
        ));
    }
    EmittedFile {
        // `_test.go` suffix so Save() override only fires during
        // `go test`. Production binary keeps the embedded Save.
        path: output_path(OutputKind::TestFile {
            file_name: "test_compat_test.go",
        })
        .path,
        content: s,
    }
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
        path: output_path(OutputKind::RoutesTable).path,
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
    // Detect whether the app emits a layouts/application view —
    // real-blog has app/views/layouts/application.html.erb (View
    // name `layouts/application`); tiny-blog doesn't ship a layout
    // at all. Without the check, the emitted dispatch.go references
    // an undefined ViewsLayouts_application symbol on `go vet`.
    let has_app_layout = app
        .views
        .iter()
        .any(|v| v.name.as_str() == "layouts/application" && v.format.as_str() == "html");
    let mut s = String::new();
    s.push_str("// Generated by Roundhouse (go2 wedge 15).\n");
    s.push_str("// Do not edit by hand — controller list is derived from app/controllers/.\n\n");
    s.push_str("package v2\n\n");
    s.push_str("import (\n\t\"net/http\"\n\t\"strings\"\n)\n\n");
    s.push_str("// Dispatch builds a controller for (controller, action), threads\n");
    s.push_str("// request metadata into it, runs the action, and returns the\n");
    s.push_str("// captured response state. POST/PATCH/PUT bodies are parsed\n");
    s.push_str("// via runtime/go/v2/form_params.go (ParseFormParams) — Rails-\n");
    s.push_str("// style bracket-notation keys like `article[title]` land as\n");
    s.push_str("// nested maps so `params[\"article\"][\"title\"]` resolves.\n");
    s.push_str(
        "func Dispatch(controller string, action string, pathParams map[string]string, r *http.Request) (body string, status int64, contentType string, location string, flash map[string]string) {\n",
    );
    s.push_str("\tparams := map[string]RoundhouseParamValue{}\n");
    s.push_str("\tfor k, v := range pathParams {\n");
    s.push_str("\t\tparams[k] = v\n");
    s.push_str("\t}\n");
    s.push_str("\tfor k, v := range ParseFormParams(r) {\n");
    s.push_str("\t\tparams[k] = v\n");
    s.push_str("\t}\n");
    s.push_str("\trequestFormat := \"html\"\n");
    s.push_str("\tif strings.HasSuffix(r.URL.Path, \".json\") {\n");
    s.push_str("\t\trequestFormat = \"json\"\n");
    s.push_str("\t}\n");
    // Reset content_for / yield slot store at dispatch entry. The
    // slots are a module-singleton package-var map in Go; Rails
    // resets them per request via ActionView's slot lifecycle. The
    // mutex emitted in library.rs::emit_module_singleton stops the
    // race-condition crash; this call ensures the data starts clean
    // each request instead of leaking content_for values across
    // sequential requests. Concurrent requests still serialize on
    // the mutex — a proper per-request scope is the right long-term
    // fix.
    s.push_str("\tActionViewViewHelpers_reset_slots_bang()\n\n");
    s.push_str("\tswitch controller {\n");
    for ctrl in &app.controllers {
        let raw = ctrl.name.0.as_str();
        let struct_name = raw.rsplit("::").next().unwrap_or(raw);
        s.push_str(&format!("\tcase {raw:?}:\n"));
        s.push_str(&format!("\t\tc := New{struct_name}()\n"));
        // Reload the flash carried from the previous request (the
        // redirect that set `flash[:notice] = …`). `ReadFlashCookie`
        // lives in the hand-written server.go; the persisted store is
        // String-keyed, matching the Flash constructor's `other` param.
        s.push_str("\t\tc.Flash = NewActionDispatchFlash(ReadFlashCookie(r))\n");
        s.push_str("\t\tc.Params = params\n");
        s.push_str("\t\tc.RequestMethod = r.Method\n");
        s.push_str("\t\tc.RequestPath = r.URL.Path\n");
        s.push_str("\t\tc.RequestFormat = requestFormat\n");
        s.push_str("\t\tc.ProcessAction(action)\n");
        // Layout wrap — only for text/html responses with non-empty
        // body, and only when the app emits a Layouts::application
        // view. Redirects (empty body, 3xx) and JSON responses skip.
        // Hardcoded to Layouts::application; multi-layout apps would
        // need per-controller layout selection (Rails `layout :foo`).
        // The has_app_layout check keeps tiny-blog (and any other
        // fixture without a layouts/application.html.erb) buildable.
        if has_app_layout {
            s.push_str("\t\tfinalBody := c.Body\n");
            s.push_str("\t\tif strings.HasPrefix(c.ContentType, \"text/html\") && c.Body != \"\" {\n");
            s.push_str("\t\t\tfinalBody = ViewsLayouts_application(c.Body, c.Flash.OpGet(\"notice\"), c.Flash.OpGet(\"alert\"))\n");
            s.push_str("\t\t}\n");
            // `to_persisted` keeps only the entries this action SET
            // (notice_was/alert_was diff) — the show-once sweep. The
            // caller (router_glue.go) writes the result back to the
            // cookie; an empty map clears it so the notice shows once.
            s.push_str("\t\treturn finalBody, c.Status, c.ContentType, c.Location, c.Flash.ToPersisted()\n");
        } else {
            s.push_str("\t\treturn c.Body, c.Status, c.ContentType, c.Location, c.Flash.ToPersisted()\n");
        }
    }
    s.push_str("\t}\n");
    s.push_str("\treturn \"\", 404, \"\", \"\", nil\n");
    s.push_str("}\n");
    EmittedFile {
        path: output_path(OutputKind::Dispatch).path,
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
            block_param: None,
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

/// Rewrite a `package app` Go file (as produced by the runtime loader
/// and per-LC emit functions) to `package <target_pkg>`, and inject a
/// per-file `import (…)` block.
///
/// `target_pkg` is the Go package name to declare; today every caller
/// passes `"v2"` (the Phase 1 invariant — flat overlay layout). Phase 4
/// flips this per file as packages split across `internal/models/`,
/// `internal/controllers/`, `pkg/runtime/*`, etc.
///
/// `extra` carries imports the caller knows about explicitly — Phase 3+
/// emit code calls `FileImports::add` to register cross-package
/// references. They are merged with the content-scan stdlib detection
/// below; the union is emitted in alphabetical order (Go convention,
/// `gofmt`-compatible).
fn rewrite_package(content: &str, target_pkg: &str, extra: &FileImports) -> String {
    let imports = collect_imports(content, extra);
    let package_line = format!("package {target_pkg}\n");
    let mut out = String::with_capacity(content.len() + 64);
    let mut saw_pkg = false;
    let mut imports_emitted = false;
    for line in content.lines() {
        if !saw_pkg && line.starts_with("package ") {
            out.push_str(&package_line);
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
        let mut prefixed = format!("{package_line}\n");
        if !imports.is_empty() {
            emit_imports(&mut prefixed, &imports);
        }
        prefixed.push_str(&out);
        return prefixed;
    }
    out
}

/// Resolve the final set of imports for a file: the union of the
/// content-scan stdlib detection and the caller's explicit `extra`,
/// sorted alphabetically. The content scan is the Phase 1 fallback —
/// Phase 3 will progressively migrate stdlib detection onto explicit
/// `FileImports::add` calls at emit sites and shrink the scan table.
fn collect_imports(content: &str, extra: &FileImports) -> Vec<String> {
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for path in scan_stdlib_imports(content) {
        set.insert(path.to_string());
    }
    for path in extra.iter() {
        set.insert(path.to_string());
    }
    set.into_iter().collect()
}

/// Content-substring scan for stdlib imports. Each entry is
/// `(probes[], package)`. A package gets imported when ANY of its
/// probes is present in `content`. Most stdlib packages have a
/// unique-enough top-level prefix (`fmt.`, `strings.`, …) that the
/// bare-period probe is safe; `time.` is an exception because the
/// file header comment "at app emit time." false-matches, so its
/// probes specifically target the call sites the peepholes emit
/// (`time.Now(`, `time.RFC3339`). Add probes here when a new
/// `time.X` emit lands.
fn scan_stdlib_imports(content: &str) -> Vec<&'static str> {
    let entries: &[(&[&str], &str)] = &[
        (&["base64."], "encoding/base64"),
        (&["cmp."], "cmp"),
        (&["fmt."], "fmt"),
        (&["html.EscapeString("], "html"),
        (&["json."], "encoding/json"),
        (&["regexp."], "regexp"),
        (&["slices."], "slices"),
        (&["sort."], "sort"),
        (&["strconv."], "strconv"),
        (&["strings."], "strings"),
        (&["sync."], "sync"),
        (&["time.Now(", "time.RFC3339"], "time"),
    ];
    let mut out = Vec::new();
    for (probes, name) in entries {
        if probes.iter().any(|p| content.contains(p)) {
            out.push(*name);
        }
    }
    out
}

fn emit_imports(out: &mut String, imports: &[String]) {
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
