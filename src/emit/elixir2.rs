//! Elixir target — `elixir2` parallel emit (Phase 1 scaffold).
//!
//! Mirrors the `go2` / `rust2` migration pattern. Strangler-fig
//! overlay that runs alongside the legacy `src/emit/elixir.rs` while
//! the migration to the lowered IR (`LibraryClass` + `MethodDef`,
//! transpiled from `runtime/ruby/`) lands.
//!
//! The overlay emits transpiled framework-runtime files under
//! `lib/v2/` inside a dedicated `*` Elixir module namespace, so it
//! can never collide with the legacy hand-written `runtime/elixir/*.ex`
//! (which live under `Roundhouse.*`) or with legacy app-emitted modules
//! (`Router`, `Post`, …). The legacy emit is otherwise untouched: it
//! still produces every `lib/*.ex` it always has.
//!
//! Phase 1 scope: scaffold + minimal transpile of the narrowest runtime
//! slice (`inflector.rb` only). Method bodies are `raise "elixir2 stub"`
//! — `mix compile --warnings-as-errors` over `lib/v2/` produces a real
//! error inventory we can drive future sessions against. The runtime
//! table (`ELIXIR_RUNTIME` in `runtime_loader.rs`) widens one file at a
//! time as the per-variant body walker in `expr.rs` grows coverage.
//!
//! Why Elixir is the high-information target: it's functional and
//! immutable, so the lowered IR's mutable-receiver assumptions
//! (`self.foo = x`, in-place `save`) can't translate directly —
//! mutation must be threaded through return values at call sites. That
//! problem doesn't bite at Phase 1 (bodies are stubs); the inventory is
//! what will quantify where it actually matters.
//!
//! Deferred to later phases: `ty.rs` (Elixir is dynamically typed —
//! Phase 1 emits no type info), models / controllers / views (those
//! stay on the legacy emitter for now), and the hand-written module-
//! state holder for `ActionView::ViewHelpers`'s `@slots`.

use super::EmittedFile;
use crate::App;

mod expr;
mod library;
mod paths;
mod test_emit;

use paths::{output_path, OutputKind};

// Re-export the library emit functions consumed by
// `runtime_loader::ELIXIR_TARGET`.
pub use library::{emit_library_class, emit_module, format_constant};

/// Append the elixir2 transpiled-runtime overlay to `files`.
pub fn overlay_v2(files: &mut Vec<EmittedFile>, app: &App) {
    files.extend(emit_overlay_files(app));
}

/// Produce the elixir2 overlay files. Phase 1: just the transpiled
/// framework runtime (`lib/v2/inflector.ex`, …), emitted
/// unconditionally — the slice has no app-dependent shims yet.
pub fn emit_overlay_files(app: &App) -> Vec<EmittedFile> {
    let mut out = Vec::new();

    // Cross-file constant resolution: register EVERY unit's `*` module
    // name before emitting any file, so a reference that crosses files
    // (e.g. `ActionController::Base` referencing `ActionDispatch::Session`
    // defined in another unit) resolves. `elixir_units` emits one file at
    // a time, so a per-unit registration alone can't see modules it
    // hasn't reached yet. Clear first so a prior emit doesn't leak.
    expr::clear_modules();
    expr::clear_field_names();
    expr::clear_param_types();
    if let Err(e) = crate::runtime_loader::elixir_library_classes(|classes| {
        expr::register_modules(classes.iter());
        // Register each class's struct fields (post-functionalize, since
        // mutation-threading is what surfaces bare-ivar fields) so a
        // method-on-typed-local can tell a field read from a method call.
        for class in crate::lower::functionalize::functionalize(classes.to_vec()) {
            let fields = library::struct_fields(&class);
            expr::register_field_names(class.name.0.as_str(), &fields);
        }
    }) {
        out.push(EmittedFile {
            path: output_path(OutputKind::TranspileError).path,
            content: format!("# elixir2 transpile failed: {e}\n"),
        });
        return out;
    }

    // Register the runtime's DECLARED module-level constant names so a
    // SCREAMING_SNAKE reference becomes a module attribute (`@escapes`)
    // only when it's actually a constant — not for an all-caps module
    // reference like `JSON` (which would otherwise become `@json`).
    expr::clear_declared_constants();
    crate::runtime_loader::elixir_constant_names(expr::register_declared_constant);

    // Register each model's struct fields WITH their schema column types,
    // for both `<Model>` and the synthesized `<Model>Row` holder. Drives
    // (a) method-on-typed-local field-vs-method routing (`row.id` →
    // `row.id`, not `row.__struct__.id(row)`) and (b) field-read type
    // dispatch (`record.title.empty?` → `record.title == ""` when
    // `title: Str`) — the body-typer's annotations don't survive the
    // functionalize passes, so the schema type is recorded here instead.
    for model in &app.models {
        if let Some(table) = app.schema.tables.get(&model.table.0) {
            // MODEL struct slots use the STORAGE name (`created_at_raw`
            // for a temporal column — the shared lowering's split); the
            // Row class is the raw transport and keeps plain column
            // names. A temporal reader's public name is NOT a slot — it
            // resolves to the emitted parsing function
            // (`Article.created_at/1` → native `%DateTime{}`).
            let mut fields: Vec<(String, crate::ty::Ty)> = table
                .columns
                .iter()
                .map(|c| {
                    (
                        crate::lower::model_to_library::col_storage_name(c).to_string(),
                        crate::lower::model_to_library::ty_of_column(&c.col_type),
                    )
                })
                .collect();
            // The AR `errors` collection is a list (not a schema column) —
            // register it as Array so `record.errors.empty?` routes to
            // `Enum.empty?` rather than a struct-method dispatch.
            fields.push((
                "errors".to_string(),
                crate::ty::Ty::Array { elem: Box::new(crate::ty::Ty::Str) },
            ));
            let name = model.name.0.as_str();
            expr::register_field_types(name, &fields);
            let row_fields: Vec<(String, crate::ty::Ty)> = table
                .columns
                .iter()
                .map(|c| {
                    (c.name.to_string(), crate::lower::model_to_library::ty_of_column(&c.col_type))
                })
                .collect();
            expr::register_field_types(&format!("{name}Row"), &row_fields);
        }
    }

    // `id` is the universal AR primary key — every model's defstruct has
    // it. Register it on the abstract `ActiveRecord::Base` so a generic
    // `record` (typed `ActiveRecord::Base`, e.g. `dom_id`'s param) reads
    // `record.id` as a struct field rather than the polymorphic
    // `record.__struct__.id(record)` dispatch (there's no `id/1` method —
    // `id` is a field). A genuine method on the generic record
    // (`record.dom_prefix()`) isn't a registered field, so it still
    // dispatches through `__struct__`.
    expr::register_field_names("ActiveRecord::Base", &["id".to_string()]);

    // Functional-target lowerings (issue #29): rewrite imperative
    // control flow (while→recursion, …) into the functional IR the
    // Elixir emitter can render directly. No-op on shapes it doesn't
    // support — those degrade via the emitter's report_unsupported
    // catch-all. Gated here: only functional emitters opt in.
    let units = match crate::runtime_loader::elixir_units(|_ns, classes| {
        // Module names are already registered globally above; this
        // re-registration is idempotent (keeps the transform self-contained
        // for any future direct caller).
        expr::register_modules(classes.iter());
        crate::lower::functionalize::functionalize(classes)
    }) {
        Ok(u) => u,
        Err(e) => {
            // Transpile failure surfaces as a sentinel file rather than
            // a panic — `mix compile` picks it up and the overlay test
            // sees a non-empty (failing) result. Mirrors go2.
            out.push(EmittedFile {
                path: output_path(OutputKind::TranspileError).path,
                content: format!("# elixir2 transpile failed: {e}\n"),
            });
            return out;
        }
    };

    for unit in &units {
        let file_name = unit
            .out_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unit.ex".to_string());
        let dest = output_path(OutputKind::TranspiledRuntime { file_name: &file_name });
        out.push(EmittedFile {
            path: dest.path,
            content: unit.content.clone(),
        });
    }

    // Native-`DateTime` seam for temporal columns: `RhDateTime.parse`
    // (the `ActiveSupport.parse_db_time` intrinsic) + the guard-clause
    // `RhDateTime.encode_datetime` every emitted
    // `JsonBuilder.encode_datetime` call routes through (native
    // %DateTime{} → Rails-canonical JSON; text/nil delegate to the
    // transpiled String variant). Ships unconditionally alongside the
    // transpiled json_builder it wraps.
    out.push(EmittedFile {
        path: output_path(OutputKind::HandWrittenRuntime { name: "rh_datetime.ex" }).path,
        content: include_str!("../../runtime/elixir/v2/rh_datetime.ex").to_string(),
    });

    // Model-support runtime: the portable prepared-cursor DB surface the
    // lowered model emit targets (`Db.prepare/step?/column_*/exec/…`),
    // a thin hand-written wrapper over `Exqlite.Sqlite3` reusing
    // `Roundhouse.Db`'s connection. Gated on models (it references
    // `Roundhouse.Db`, only emitted when the app has models). Mirrors
    // go2's `db.go` / rust2's `db.rs` hand-written-runtime rationale.
    if !app.models.is_empty() {
        out.push(EmittedFile {
            path: output_path(OutputKind::HandWrittenRuntime { name: "db.ex" }).path,
            content: include_str!("../../runtime/elixir/v2/db.ex").to_string(),
        });
        // Turbo Streams broadcasts shim — the model after_*_commit
        // callbacks call `Broadcasts.<action>(%{…})`. Hand-written like
        // the sibling per-target shims (the canonical broadcasts.rb
        // doesn't translate to Elixir's immutable model). See db.ex for
        // the hand-written-runtime rationale.
        out.push(EmittedFile {
            path: output_path(OutputKind::HandWrittenRuntime { name: "broadcasts.ex" }).path,
            content: include_str!("../../runtime/elixir/v2/broadcasts.ex").to_string(),
        });

        // Per-model emit. Elixir has no inheritance, so each model module
        // is standalone: the lowered model LC (defstruct + per-model
        // _adapter_* / from_row / validate / …) gets the AR-baseline
        // method BODIES (find/all/save/destroy/count/…) materialized in
        // from `active_record/base.rb` — the linearization the other
        // targets get via embedding/traits. (See "how does Phoenix"
        // resolution: schema-module + Repo, no Base module.)
        //
        // Phase D (2026-06-05): emitted unconditionally — v2 is the default
        // elixir target. real-blog + tiny-blog (both with associations)
        // compile clean and reach byte-parity with Rails. An app whose
        // has_many getter hits the not-yet-recursion-lowered `while`-in-`if`
        // hydration shape would still fail to mix-compile; no such fixture
        // is in CI, and the gap is tracked for Phase-D follow-up.
        // The lowered model emit references the DB primitive as bare
        // `Db.prepare`/`Db.step?`/… — resolve those to the hand-written
        // `Db` module (db.ex above). Likewise `Broadcasts.<action>`
        // (model after_*_commit callbacks) → the `Broadcasts` shim.
        expr::register_module("Db", "Db");
        expr::register_module("Broadcasts", "Broadcasts");
        let base_methods = ar_base_methods();
        // The model dual `{record, value}` methods (`save`/`valid?`/
        // `destroy`, and the per-model synthesized `update` whose tail
        // `save` propagates dual) — controllers call these on a model-typed
        // field (`@article.save`/`.update`), and their field-receiver call
        // sites must destructure the tuple rather than test it whole
        // (always truthy). Seeded from the AR baseline; extended below with
        // each materialized model's methods (which carry `update`).
        let mut model_duals =
            crate::lower::functionalize::mutation_to_struct_return::dual_method_names(&base_methods);
        // Strong-params specs from the controllers: each `permit(...)`
        // declares a `<Resource>Params` factory the model lowerer wires
        // into `Model.from_params(...)` (controller `Model.new(
        // article_params)` call sites rewrite to it). Empty when
        // controllers aren't being emitted, but harmless to always
        // collect. Mirrors go2.rs:260-268.
        let specs: std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>> =
            crate::lower::controller_to_library::params::collect_specs(&app.controllers)
                .into_iter()
                .map(|(r, s)| (r, s.fields))
                .collect();
        // Register each synthesized `<Resource>Params` struct's fields up
        // front — the model `from_params(p)` reads `p.title`/`p.body`, and
        // without the field registry those typed-local reads mis-route to
        // a `p.__struct__.title(p)` method dispatch (an undefined fn). The
        // model emit runs before the controllers block, so this can't wait
        // for the controller field registration there.
        for (resource, fields) in &specs {
            let class_name = format!("{}Params", crate::naming::camelize(resource.as_str()));
            let field_names: Vec<String> =
                fields.iter().map(|f| f.as_str().to_string()).collect();
            expr::register_field_names(&class_name, &field_names);
        }
        let (model_lcs, model_registry) =
            crate::lower::model_to_library::lower_models_with_registry_and_params(
                &app.models,
                &app.schema,
                vec![],
                &specs,
            );
        expr::register_modules(model_lcs.iter());
        let model_names: std::collections::HashSet<String> =
            app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();

        // Build the view layer (RouteHelpers + Importmap + per-resource
        // `Views::<Resource>` modules) UP FRONT — before model emit — so a
        // model after_*_commit broadcast ref (`Views::Articles.article(
        // record)`) resolves to the registered `Views.Articles`.
        // Mirrors go2's lower-all → register-all → emit-all order
        // (go2.rs:313-400). Gated on views (their only consumer today;
        // controllers, the other RouteHelpers caller, aren't yet v2-emitted).
        let views_enabled = !app.views.is_empty();
        let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
        let mut view_layer_lcs: Vec<crate::dialect::LibraryClass> = Vec::new();
        if views_enabled {
            // RouteHelpers (`<name>_path(args)`) + Importmap (`pins`/`entry`)
            // — lowered into module-flavored LCs, the same library-emit
            // pipeline as models/views (→ `RouteHelpers`/`Importmap`,
            // self-contained so v1's `Roundhouse.*` can be retired).
            if !route_helper_funcs.is_empty() {
                view_layer_lcs
                    .push(module_funcs_to_library_class("RouteHelpers", &route_helper_funcs));
            }
            let importmap_funcs = crate::lower::lower_importmap_to_library_functions(app);
            if !importmap_funcs.is_empty() {
                view_layer_lcs
                    .push(module_funcs_to_library_class("Importmap", &importmap_funcs));
            }
            // Per-resource Views (HTML + JBuilder merged by struct name),
            // typed against the model registry AND the route helpers (so a
            // partial's `RouteHelpers.article_path(article.id)` arg types).
            let mut view_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
                model_registry.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            view_extras
                .extend(crate::lower::library_extras::extras_from_funcs(&route_helper_funcs));
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
            // Merge HTML + JSON variants by struct name (one
            // `Views::<Resource>` module per resource — go2.rs:369).
            let mut merged: std::collections::BTreeMap<String, crate::dialect::LibraryClass> =
                std::collections::BTreeMap::new();
            for lc in raw_lcs {
                let raw = lc.name.0.as_str();
                let struct_name = raw.rsplit("::").next().unwrap_or(raw).to_string();
                merged
                    .entry(struct_name)
                    .and_modify(|acc: &mut crate::dialect::LibraryClass| {
                        let seen: std::collections::BTreeSet<String> =
                            acc.methods.iter().map(|m| m.name.as_str().to_string()).collect();
                        for m in lc.methods.clone() {
                            if !seen.contains(m.name.as_str()) {
                                acc.methods.push(m);
                            }
                        }
                    })
                    .or_insert(lc);
            }
            view_layer_lcs.extend(merged.into_values());
            // Register ALL view-layer modules before model emit so cross-
            // refs (model→Views, view→RouteHelpers/Importmap) resolve.
            expr::register_modules(view_layer_lcs.iter());

            // Type each view partial's record param. A partial param named
            // after a model's snake_case singular (`article`) is that model
            // (`Class{Article}`); the plural (`articles`) is a list of it
            // (`Array{Article}`). Without this the param is untyped, so
            // `article.errors` can't resolve `errors` to `Array` and the
            // form's `article.errors.count`/`.empty?` mis-route to a struct
            // field/`__struct__` dispatch on a list (a runtime crash on
            // new/edit). The view lowering knows resource→model; we mirror
            // it here from the model registry.
            let model_param_types: Vec<(String, crate::ty::Ty)> = model_names
                .iter()
                .map(|m| {
                    let cls = crate::ty::Ty::Class {
                        id: crate::ident::ClassId(m.clone().into()),
                        args: vec![],
                    };
                    (crate::naming::snake_case(m), cls)
                })
                .flat_map(|(singular, cls)| {
                    let plural = crate::naming::pluralize_snake(&singular);
                    let list = crate::ty::Ty::Array { elem: Box::new(cls.clone()) };
                    [(singular, cls), (plural, list)]
                })
                .collect();
            for lc in &view_layer_lcs {
                // Only register a name that is actually a param of some
                // method in this view module — so the type lands only where
                // a partial threads that record, not on every view module.
                let params: Vec<(String, crate::ty::Ty)> = model_param_types
                    .iter()
                    .filter(|(name, _)| {
                        lc.methods.iter().any(|mth| mth.params.iter().any(|p| p.as_str() == name))
                    })
                    .cloned()
                    .collect();
                if !params.is_empty() {
                    expr::register_param_types(lc.name.0.as_str(), &params);
                }
            }
        }

        for mut lc in model_lcs {
            // Skip the abstract `ApplicationRecord` base — concrete models
            // are self-contained after materialization, so nothing
            // instantiates it; emitting it would surface its lowering-added
            // `create`/`create!` (which call a `new` it lacks).
            if lc.name.0.as_str() == "ApplicationRecord" {
                continue;
            }
            // Only concrete models get the AR-baseline CRUD materialized —
            // not the `<Model>Row` data holders, which have no `new`/table
            // and would get a `create` calling an absent `new`.
            if model_names.contains(lc.name.0.as_str()) {
                materialize_inherited(&mut lc, &base_methods);
                // Pick up per-model dual methods (the synthesized `update`,
                // dual by tail-call propagation to `save`) for the
                // controller field-receiver call sites (`if @article.update`).
                model_duals.extend(
                    crate::lower::functionalize::mutation_to_struct_return::dual_method_names(
                        &lc.methods,
                    ),
                );
            }
            emit_library_lc(lc, &mut out);
        }
        for lc in view_layer_lcs {
            emit_library_lc(lc, &mut out);
        }

        // ---- Controllers (Phase C / W1) -----------------------------
        // Per-controller app modules (`ArticlesController`, …) lowered
        // from the same `controller_to_library` pipeline go2/rust2 use.
        // Elixir has no inheritance, so the `ActionController::Base`
        // methods (initialize/render/redirect_to/head/resolve_status) are
        // MATERIALIZED into each concrete controller — the same
        // linearization the model emit does with the AR baseline.
        // Phase D: emitted unconditionally (v2 default); requires views
        // (action bodies render `Views.*`).
        let controllers_enabled = !app.controllers.is_empty();
        if controllers_enabled {
            let model_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
                model_registry.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            // Drop `includes(:assoc)` eager-loads for now (pass an EMPTY
            // association graph): the assoc-graph path inlines a second
            // cursor-drain + a preload-grouping `each` per `includes`,
            // which the functionalize passes don't yet lower cleanly. The
            // lazy `comments` has_many getter already works, so dropping
            // the eager-load is behaviorally correct (N+1, not wrong) — an
            // optimization deferral, not a feature gap. Re-enable with
            // `compute_association_graph(app)` once the eager-load IR
            // (multi-drain + preload-each) is functionalized.
            let raw_controller_lcs =
                crate::lower::controller_to_library::lower_controllers_with_arel_and_views(
                    &app.controllers,
                    model_extras,
                    Some(&app.schema),
                    &app.views,
                );
            // Materialize the AC baseline into each concrete controller
            // FIRST — the base `initialize` is what seeds the response-state
            // struct fields (params/session/flash/request_format/status/…),
            // so field registration (below) must see the materialized form
            // or `record.request_format` accessor reads mis-route to an
            // undefined 0-arg call. Drop the abstract `ApplicationController`
            // (class macros only; nothing instantiates it once concrete
            // controllers are self-contained — mirrors `ApplicationRecord`).
            let ac_base = ac_base_methods();
            let controller_lcs: Vec<crate::dialect::LibraryClass> = raw_controller_lcs
                .into_iter()
                .filter(|lc| lc.name.0.as_str() != "ApplicationController")
                .map(|mut lc| {
                    // Concrete controllers get the AC baseline; the
                    // synthesized `<Resource>Params` holders do not (no
                    // actions / response state).
                    if lc.name.0.as_str().ends_with("Controller") {
                        materialize_controller_inherited(&mut lc, &ac_base);
                    }
                    lc
                })
                .collect();
            // Register every controller (+ `<Resource>Params`) module name
            // and (post-materialization, post-functionalize) struct fields
            // BEFORE emit, so cross-refs and field-vs-method routing resolve.
            expr::register_modules(controller_lcs.iter());
            for class in crate::lower::functionalize::functionalize_with_external_duals(
                controller_lcs.clone(),
                &model_duals,
            ) {
                // Register fields WITH types: the response-state structs
                // (`flash`/`session`) carry their class type so a
                // `flash[:notice]` indexer routes to the renamed accessor
                // (`ActionDispatch.Flash.get`) rather than raw `Access`
                // (which structs don't implement). The rest stay Untyped.
                let typed: Vec<(String, crate::ty::Ty)> = library::struct_fields(&class)
                    .into_iter()
                    .map(|f| {
                        let ty = match f.as_str() {
                            "flash" => class_ty("ActionDispatch::Flash"),
                            "session" => class_ty("ActionDispatch::Session"),
                            // A field named after a model (`@article`,
                            // `@comment`) holds that model — type it so
                            // `record.article.save` routes to a method call
                            // (`record.article.__struct__.save(...)`) rather
                            // than a `.save` field access (KeyError). Plural
                            // collection fields (`@articles`) don't match a
                            // model name and stay Untyped (they're lists).
                            other => {
                                let camel = crate::naming::camelize(other);
                                if model_names.contains(&camel) {
                                    class_ty(&camel)
                                } else {
                                    crate::ty::Ty::Untyped
                                }
                            }
                        };
                        (f, ty)
                    })
                    .collect();
                expr::register_field_types(class.name.0.as_str(), &typed);
            }
            let before = out.len();
            for lc in controller_lcs {
                emit_library_lc_with_duals(lc, &mut out, &model_duals);
            }
            // The materialized `resolve_status` reads the `STATUS_CODES`
            // table, which lives as a module-level constant in
            // `action_controller/base.rb` (→ `@status_codes` in
            // `ActionController.Base`). It doesn't travel with the
            // copied MethodDef, so inject it as a module attribute into any
            // emitted controller that references it. (Module constants
            // aren't carried on `LibraryClass`; this mirrors how the
            // runtime loader injects module constants for runtime files.)
            if let Some(attr) = status_codes_attr_line() {
                for file in out.iter_mut().skip(before) {
                    if file.content.contains("@status_codes")
                        && !file.content.contains(&attr)
                    {
                        file.content = inject_module_attr(&file.content, &attr);
                    }
                }
            }

            // W2 — the route table (`RoutesTable.table/0`): a list of
            // `ActionDispatch.Router.Route` structs the (stateless)
            // router `match/3` scans. Controller is the string name the
            // dispatch shim (W3) maps to `<Name>Controller`; action is
            // the atom `process_action` dispatches on.
            out.push(emit_routes_table_file(app));

            // W3 — the dispatch shim (`Dispatch.call/5`): build the
            // controller struct, thread the request params/format, run
            // `process_action`, wrap HTML in the layout, return the
            // captured response state.
            out.push(emit_dispatch_file(app));

            // W6 — the Plug/Cowboy HTTP server (`Server`), hand-written
            // like db.ex/broadcasts.ex. Dispatches through the v2 router +
            // `Dispatch`. (Reuses `Roundhouse.Db`'s connection during
            // the strangler phase — `Db` already does.)
            out.push(EmittedFile {
                path: output_path(OutputKind::HandWrittenRuntime { name: "server.ex" }).path,
                content: include_str!("../../runtime/elixir/v2/server.ex").to_string(),
            });

            // W5 — the boot entry (`Main.run/0`): open the DB + run the
            // server. The compare/boot driver invokes it via `mix run -e`.
            out.push(emit_main_file());

            // W7 (Phase D2) — the v2 ExUnit test tree: `TestClient`/
            // `TestResponse` runtime, v2 fixtures, and per-controller
            // test modules under `test/v2/`. Distinct module names + paths
            // from the v1 tree, so both suites run under `mix test` during
            // the strangler phase. Model tests deferred (dual-return `save`
            // call sites need functionalize support — see test_emit docs).
            out.extend(test_emit::emit_test_files(app));
        }
    }

    out
}

/// Emit `lib/v2/main.ex` — `Main.run/0`, the boot entry. Opens the DB
/// (schema via the legacy `Roundhouse.SchemaSQL`, reused during the
/// strangler phase) and runs `Server` (which blocks on Plug.Cowboy).
fn emit_main_file() -> EmittedFile {
    let content = "# Generated by Roundhouse (elixir2 Phase C / W5).\n\
         defmodule Main do\n\
         \x20 @moduledoc false\n\n\
         \x20 def run do\n\
         \x20   Server.start(Roundhouse.SchemaSQL.create_tables())\n\
         \x20 end\nend\n"
        .to_string();
    EmittedFile {
        path: output_path(OutputKind::Main).path,
        content,
    }
}

/// Emit `lib/v2/dispatch.ex` — `Dispatch.call/5`. One `case` arm per
/// concrete controller (mirrors go2's `emit_dispatch_file`): construct the
/// controller struct, set `params`/`request_format`, run `process_action`,
/// then wrap a non-empty HTML body in the app layout (when one exists) and
/// return `{body, status, content_type, location}`.
fn emit_dispatch_file(app: &App) -> EmittedFile {
    // Layout wrap only when the app ships `app/views/layouts/application`
    // (real-blog does; a layout-less app would reference an undefined
    // `Views.Layouts`).
    let has_app_layout = app
        .views
        .iter()
        .any(|v| v.name.as_str() == "layouts/application" && v.format.as_str() == "html");
    let finalize = if has_app_layout {
        // The layout's `notice`/`alert` params are unused (the flash is
        // rendered inside the per-action partial), so pass `nil`. The 5th
        // tuple element is the flash to carry to the next request —
        // `to_persisted` keeps only what the action SET (show-once sweep);
        // `Server.dispatch` writes it to the rh_flash cookie.
        "    body =\n\
         \x20     if String.starts_with?(c.content_type, \"text/html\") and c.body != \"\" do\n\
         \x20       Views.Layouts.application(c.body, nil, nil)\n\
         \x20     else\n\
         \x20       c.body\n\
         \x20     end\n\
         \x20   {body, c.status, c.content_type, c.location, ActionDispatch.Flash.to_persisted(c.flash)}\n"
    } else {
        "    {c.body, c.status, c.content_type, c.location, ActionDispatch.Flash.to_persisted(c.flash)}\n"
    };

    let mut arms = String::new();
    for ctrl in &app.controllers {
        let raw = ctrl.name.0.as_str();
        if raw == "ApplicationController" {
            continue;
        }
        let module = library::v2_module_name(raw);
        arms.push_str(&format!(
            "      {raw:?} ->\n\
             \x20       c = {module}.new()\n\
             \x20       c = %{{c | params: params, request_format: request_format, flash: ActionDispatch.Flash.new(incoming_flash)}}\n\
             \x20       c = {module}.process_action(c, action)\n\
             {finalize}"
        ));
    }

    let content = format!(
        "# Generated by Roundhouse (elixir2 Phase C / W3).\n\
         # Do not edit by hand — controller list is derived from app/controllers/.\n\n\
         defmodule Dispatch do\n\
         \x20 @doc \"\"\"\n\
         \x20 Build a controller for `(controller, action)`, thread request\n\
         \x20 params + format + incoming flash into it, run the action, and\n\
         \x20 return the captured `{{body, status, content_type, location,\n\
         \x20 flash}}` response state, where `flash` is the String-keyed map\n\
         \x20 to carry to the next request (`Flash.to_persisted`).\n\
         \x20 `request_format` is an atom (`:html` / `:json`); `path_params`,\n\
         \x20 `body_params`, and `incoming_flash` are string-keyed maps (body\n\
         \x20 params nested, e.g. `%{{\"article\" => %{{\"title\" => …}}}}`).\n\
         \x20 \"\"\"\n\
         \x20 def call(controller, action, path_params, body_params, request_format, incoming_flash) do\n\
         \x20   params = Map.merge(path_params, body_params)\n\
         \x20   case controller do\n\
         {arms}\
         \x20     _ ->\n\
         \x20       {{\"Not Found\", 404, \"text/plain\", nil, %{{}}}}\n\
         \x20   end\n  end\nend\n"
    );
    EmittedFile {
        path: output_path(OutputKind::Dispatch).path,
        content,
    }
}

/// Emit `lib/v2/routes_table.ex` — `RoutesTable.table/0` returning the
/// flattened route list (one `ActionDispatch.Router.Route` per route).
fn emit_routes_table_file(app: &App) -> EmittedFile {
    use crate::dialect::HttpMethod;
    let mut entries = String::new();
    for r in crate::lower::flatten_routes(app) {
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
        entries.push_str(&format!(
            "      ActionDispatch.Router.Route.new({verb:?}, {path:?}, {ctrl:?}, :{action}),\n",
            path = r.path,
            ctrl = r.controller.0.as_str(),
            action = r.action.as_str(),
        ));
    }
    let content = format!(
        "# Generated by Roundhouse (elixir2 Phase C / W2).\n\
         # Do not edit by hand — edit `config/routes.rb` and re-run emit.\n\n\
         defmodule RoutesTable do\n\
         \x20 @doc \"Every Rails-style route the app exposes, in match order.\"\n\
         \x20 def table do\n\
         \x20   [\n{entries}    ]\n  end\nend\n"
    );
    EmittedFile {
        path: output_path(OutputKind::RoutesTable).path,
        content,
    }
}

/// A nullary `Ty::Class` for `class` (e.g. `ActionDispatch::Flash`).
fn class_ty(class: &str) -> crate::ty::Ty {
    crate::ty::Ty::Class {
        id: crate::ident::ClassId(crate::ident::Symbol::from(class)),
        args: Vec::new(),
    }
}

/// The `@status_codes %{...}` module-attribute line, rendered from
/// `action_controller/base.rb`'s `STATUS_CODES` constant — injected into
/// controllers whose materialized `resolve_status` references it.
fn status_codes_attr_line() -> Option<String> {
    let consts = crate::runtime_src::parse_module_constant_exprs(include_str!(
        "../../runtime/ruby/action_controller/base.rb"
    ))
    .ok()?;
    let (_, value) = consts.into_iter().find(|(n, _)| n.as_str() == "STATUS_CODES")?;
    Some(format!("  @status_codes {}", expr::emit_const_value(&value)))
}

/// Insert a module attribute line just after the `defmodule … do` header
/// of an emitted `.ex` module.
fn inject_module_attr(content: &str, attr_line: &str) -> String {
    match content.find(" do\n") {
        Some(idx) => {
            let split = idx + " do\n".len();
            format!("{}{attr_line}\n{}", &content[..split], &content[split..])
        }
        None => content.to_string(),
    }
}

/// Functionalize a `LibraryClass` and append its emitted `.ex` file(s)
/// to `out` (one per sibling module the LC expands to). A failed emit
/// becomes a visible `# emit_library_class FAILED` sentinel rather than
/// a silently-dropped file, so `mix compile` surfaces the gap.
fn emit_library_lc(lc: crate::dialect::LibraryClass, out: &mut Vec<EmittedFile>) {
    emit_library_lc_with_duals(lc, out, &std::collections::HashSet::new());
}

/// Like [`emit_library_lc`], but seeds the functionalize pass with
/// `external_duals` — dual `{record, value}` methods from OTHER classes
/// (a model's `save`/`destroy`) so a controller's field-receiver call
/// sites (`@article.save`) destructure the tuple. Models/views pass an
/// empty set (their dual methods are classified per-class).
fn emit_library_lc_with_duals(
    lc: crate::dialect::LibraryClass,
    out: &mut Vec<EmittedFile>,
    external_duals: &std::collections::HashSet<String>,
) {
    for class in
        crate::lower::functionalize::functionalize_with_external_duals(vec![lc], external_duals)
    {
        let file = format!(
            "{}.ex",
            class.name.0.as_str().to_lowercase().replace("::", "_")
        );
        let content = match library::emit_library_class(&class) {
            Ok(c) => c,
            Err(e) => format!("# emit_library_class FAILED: {e}\n"),
        };
        out.push(EmittedFile {
            path: output_path(OutputKind::TranspiledRuntime { file_name: &file }).path,
            content,
        });
    }
}

/// Bundle a set of module-level `LibraryFunction`s (route helpers,
/// importmap) into a module-flavored `LibraryClass`, so the same
/// `emit_library_class` pipeline produces `<Name>` with bare class
/// functions. Mirrors `go2.rs::module_funcs_to_library_class`.
fn module_funcs_to_library_class(
    name: &str,
    funcs: &[crate::dialect::LibraryFunction],
) -> crate::dialect::LibraryClass {
    use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
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
        name: crate::ident::ClassId(crate::ident::Symbol::from(name)),
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods,
        origin: None,
        constants: Vec::new(),
    }
}

/// The AR-baseline method definitions from `active_record/base.rb` (the
/// `ActiveRecord::Base` class body) — `find`/`all`/`save`/`destroy`/
/// `count`/lifecycle hooks/etc. Materialized into each model module
/// since Elixir has no inheritance to carry them.
fn ar_base_methods() -> Vec<crate::dialect::MethodDef> {
    crate::runtime_src::parse_library_with_rbs(
        include_str!("../../runtime/ruby/active_record/base.rb").as_bytes(),
        include_str!("../../runtime/ruby/active_record/base.rbs"),
        "runtime/ruby/active_record/base.rb",
    )
    .unwrap_or_default()
    .into_iter()
    .find(|c| c.name.0.as_str() == "ActiveRecord::Base")
    .map(|c| c.methods)
    .unwrap_or_default()
}

/// The AC-baseline method definitions from `action_controller/base.rb`
/// (the `ActionController::Base` class body) — `initialize`/`render`/
/// `redirect_to`/`head`/`resolve_status`. Materialized into each concrete
/// controller module since Elixir has no inheritance to carry them. (This
/// is the same file already transpiled standalone as
/// `ActionController.Base`; materializing rather than delegating keeps
/// each controller a self-contained struct, mirroring the model path.)
fn ac_base_methods() -> Vec<crate::dialect::MethodDef> {
    crate::runtime_src::parse_library_with_rbs(
        include_str!("../../runtime/ruby/action_controller/base.rb").as_bytes(),
        include_str!("../../runtime/ruby/action_controller/base.rbs"),
        "runtime/ruby/action_controller/base.rb",
    )
    .unwrap_or_default()
    .into_iter()
    .find(|c| c.name.0.as_str() == "ActionController::Base")
    .map(|c| c.methods)
    .unwrap_or_default()
}

/// Append the AR-baseline methods the model doesn't already override
/// onto its LibraryClass, re-homing each to the model so self-calls and
/// `Module#name` reflection resolve to the model. `initialize` is
/// excluded — the model emits its own `new`. `find_by`/`where` are
/// excluded too: they delegate to the legacy `ActiveRecord.adapter`
/// (which the Elixir target doesn't model), not the per-model Arel
/// `_adapter_*` primitives — so materializing them would reference an
/// absent adapter. (Real-blog doesn't call them; an Arel-based find_by/
/// where would be a model-lowering addition.)
/// Append the AC-baseline methods a controller doesn't already define.
/// Unlike the model variant, `initialize` IS materialized — the
/// controller has no constructor of its own, and the base `initialize`
/// is what seeds the response-state struct fields (params/session/flash/
/// status/body/location/…) the `defstruct` is derived from. The lowerer
/// synthesizes each controller's own `process_action`, so the base's
/// abstract one is filtered by the `defined` check.
fn materialize_controller_inherited(
    ctrl: &mut crate::dialect::LibraryClass,
    base: &[crate::dialect::MethodDef],
) {
    let defined: std::collections::HashSet<String> =
        ctrl.methods.iter().map(|m| m.name.as_str().to_string()).collect();
    let owner = ctrl.name.0.clone();
    let mut inherited: Vec<crate::dialect::MethodDef> = base
        .iter()
        .filter(|m| !defined.contains(m.name.as_str()))
        .cloned()
        .map(|mut m| {
            m.enclosing_class = Some(owner.clone());
            m
        })
        .collect();
    ctrl.methods.append(&mut inherited);
}

fn materialize_inherited(model: &mut crate::dialect::LibraryClass, base: &[crate::dialect::MethodDef]) {
    let defined: std::collections::HashSet<String> =
        model.methods.iter().map(|m| m.name.as_str().to_string()).collect();
    let owner = model.name.0.clone();
    // `find_by`/`where` are dropped because Elixir rewrites their call
    // sites to Ecto/Repo rather than defining the method; `find_by!`'s
    // body calls `find_by`, so it must be dropped alongside it (an Elixir
    // target that actually consumes `find_by!` would rewrite its call
    // sites to `Repo.get_by!`, same as `find_by` → `get_by`).
    let skip = ["initialize", "find_by", "find_by!", "where"];
    let mut inherited: Vec<crate::dialect::MethodDef> = base
        .iter()
        .filter(|m| !skip.contains(&m.name.as_str()) && !defined.contains(m.name.as_str()))
        .cloned()
        .map(|mut m| {
            m.enclosing_class = Some(owner.clone());
            m
        })
        .collect();
    model.methods.append(&mut inherited);
}
