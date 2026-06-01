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
        let base_methods = ar_base_methods();
        let specs: std::collections::BTreeMap<crate::ident::Symbol, Vec<crate::ident::Symbol>> =
            std::collections::BTreeMap::new();
        let (model_lcs, _registry) =
            crate::lower::model_to_library::lower_models_with_registry_and_params(
                &app.models,
                &app.schema,
                vec![],
                &specs,
            );
        expr::register_modules(model_lcs.iter());
        let model_names: std::collections::HashSet<String> =
            app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
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
            for class in crate::lower::functionalize::functionalize(vec![lc]) {
                if let Ok(content) = library::emit_library_class(&class) {
                    let file = format!(
                        "{}.ex",
                        class.name.0.as_str().to_lowercase().replace("::", "_")
                    );
                    out.push(EmittedFile {
                        path: output_path(OutputKind::TranspiledRuntime { file_name: &file }).path,
                        content,
                    });
                }
            }
        }
    }

    out
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

/// Append the AR-baseline methods the model doesn't already override
/// onto its LibraryClass, re-homing each to the model so self-calls and
/// `Module#name` reflection resolve to the model. `initialize` is
/// excluded — the model emits its own `new`.
fn materialize_inherited(model: &mut crate::dialect::LibraryClass, base: &[crate::dialect::MethodDef]) {
    let defined: std::collections::HashSet<String> =
        model.methods.iter().map(|m| m.name.as_str().to_string()).collect();
    let owner = model.name.0.clone();
    let mut inherited: Vec<crate::dialect::MethodDef> = base
        .iter()
        .filter(|m| m.name.as_str() != "initialize" && !defined.contains(m.name.as_str()))
        .cloned()
        .map(|mut m| {
            m.enclosing_class = Some(owner.clone());
            m
        })
        .collect();
    model.methods.append(&mut inherited);
}
