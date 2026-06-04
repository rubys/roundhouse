//! Elixir target — `elixir2` parallel emit (Phase 1 scaffold).
//!
//! Mirrors the `go2` / `rust2` migration pattern. Strangler-fig
//! overlay that runs alongside the legacy `src/emit/elixir.rs` while
//! the migration to the lowered IR (`LibraryClass` + `MethodDef`,
//! transpiled from `runtime/ruby/`) lands.
//!
//! The overlay emits transpiled framework-runtime files under
//! `lib/v2/` inside a dedicated `V2.*` Elixir module namespace, so it
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

    // Cross-file constant resolution: register EVERY unit's `V2.*` module
    // name before emitting any file, so a reference that crosses files
    // (e.g. `ActionController::Base` referencing `ActionDispatch::Session`
    // defined in another unit) resolves. `elixir_units` emits one file at
    // a time, so a per-unit registration alone can't see modules it
    // hasn't reached yet. Clear first so a prior emit doesn't leak.
    expr::clear_modules();
    expr::clear_field_names();
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
            let fields: Vec<(String, crate::ty::Ty)> = table
                .columns
                .iter()
                .map(|c| {
                    (c.name.to_string(), crate::lower::model_to_library::ty_of_column(&c.col_type))
                })
                .collect();
            let name = model.name.0.as_str();
            expr::register_field_types(name, &fields);
            expr::register_field_types(&format!("{name}Row"), &fields);
        }
    }

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

    let views_enabled = std::env::var("RH_ELIXIR2_VIEWS").is_ok();
    for unit in &units {
        let file_name = unit
            .out_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unit.ex".to_string());
        // `view_helpers.ex` is parsed + registered unconditionally (so the
        // view emit's `ActionView::ViewHelpers.*` references resolve), but
        // only EMITTED under the views gate while its transpile is being
        // driven to a clean `mix compile` — mirrors the models/views gate.
        if file_name == "view_helpers.ex" && !views_enabled {
            continue;
        }
        let dest = output_path(OutputKind::TranspiledRuntime { file_name: &file_name });
        out.push(EmittedFile {
            path: dest.path,
            content: unit.content.clone(),
        });
    }

    // Model-support runtime: the portable prepared-cursor DB surface the
    // lowered model emit targets (`V2.Db.prepare/step?/column_*/exec/…`),
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
        // Env-gated (off by default) while one gap remains: the has_many
        // getter's `while`-in-`if`-branch hydration loop isn't yet
        // recursion-lowered, so the model files don't all mix-compile.
        // Gating keeps the default toolchain test green (mirrors go2's
        // overlay env-gate); flip `RH_ELIXIR2_MODELS` to emit + iterate.
        if std::env::var("RH_ELIXIR2_MODELS").is_err() {
            return out;
        }
        // The lowered model emit references the DB primitive as bare
        // `Db.prepare`/`Db.step?`/… — resolve those to the hand-written
        // `V2.Db` module (db.ex above). Likewise `Broadcasts.<action>`
        // (model after_*_commit callbacks) → the `V2.Broadcasts` shim.
        expr::register_module("Db", "V2.Db");
        expr::register_module("Broadcasts", "V2.Broadcasts");
        let base_methods = ar_base_methods();
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
        // record)`) resolves to the registered `V2.Views.Articles`.
        // Mirrors go2's lower-all → register-all → emit-all order
        // (go2.rs:313-400). Gated on views (their only consumer today;
        // controllers, the other RouteHelpers caller, aren't yet v2-emitted).
        let views_enabled = std::env::var("RH_ELIXIR2_VIEWS").is_ok() && !app.views.is_empty();
        let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
        let mut view_layer_lcs: Vec<crate::dialect::LibraryClass> = Vec::new();
        if views_enabled {
            // RouteHelpers (`<name>_path(args)`) + Importmap (`pins`/`entry`)
            // — lowered into module-flavored LCs, the same library-emit
            // pipeline as models/views (→ `V2.RouteHelpers`/`V2.Importmap`,
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
            }
            emit_library_lc(lc, &mut out);
        }
        for lc in view_layer_lcs {
            emit_library_lc(lc, &mut out);
        }

        // ---- Controllers (Phase C / W1) -----------------------------
        // Per-controller app modules (`V2.ArticlesController`, …) lowered
        // from the same `controller_to_library` pipeline go2/rust2 use.
        // Elixir has no inheritance, so the `ActionController::Base`
        // methods (initialize/render/redirect_to/head/resolve_status) are
        // MATERIALIZED into each concrete controller — the same
        // linearization the model emit does with the AR baseline.
        // Env-gated (RH_ELIXIR2_CONTROLLERS) while the controller action
        // bodies are driven to a clean `mix compile`; requires views
        // (action bodies render `V2.Views.*`).
        let controllers_enabled =
            std::env::var("RH_ELIXIR2_CONTROLLERS").is_ok() && !app.controllers.is_empty();
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
            for class in crate::lower::functionalize::functionalize(controller_lcs.clone()) {
                let fields = library::struct_fields(&class);
                expr::register_field_names(class.name.0.as_str(), &fields);
            }
            let before = out.len();
            for lc in controller_lcs {
                emit_library_lc(lc, &mut out);
            }
            // The materialized `resolve_status` reads the `STATUS_CODES`
            // table, which lives as a module-level constant in
            // `action_controller/base.rb` (→ `@status_codes` in
            // `V2.ActionController.Base`). It doesn't travel with the
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
        }
    }

    out
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
    for class in crate::lower::functionalize::functionalize(vec![lc]) {
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
/// `emit_library_class` pipeline produces `V2.<Name>` with bare class
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
/// `V2.ActionController.Base`; materializing rather than delegating keeps
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
    let skip = ["initialize", "find_by", "where"];
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
