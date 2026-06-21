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

    // The synthesized `<Resource>Params` classes (which models reference in
    // `update`/`from_params`) come from the controller lowering, origin-tagged
    // to route to `app/models`. Phase 2 lowers controllers only to harvest
    // these — the real controllers land in Phase 3, so non-origin classes are
    // dropped here.
    let mut controller_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    controller_extras.extend(crate::lower::extras_from_lcs(&preliminary_views));
    let assocs = crate::lower::model_associations::compute_association_graph(app);
    let controller_lcs = crate::lower::lower_controllers_with_arel_views_and_assocs(
        &app.controllers,
        controller_extras,
        Some(&app.schema),
        &app.views,
        &assocs,
    );
    let params_lcs: Vec<crate::dialect::LibraryClass> =
        controller_lcs.into_iter().filter(|lc| lc.origin.is_some()).collect();
    library::register_class_hierarchy(&params_lcs);
    for lc in &params_lcs {
        files.push(library::emit_class_file(lc));
    }

    // Phase-2 view stubs: the broadcast callbacks (`Broadcasts.prepend(...,
    // Articles.article(this))`) reference view modules that aren't emitted
    // until Phase 4. Stub each referenced module so the model layer compiles.
    if let Some(stub) = emit_view_stubs(&preliminary_views) {
        files.push(stub);
    }

    files
}

/// Emit `app/views/ViewStubs.cs` — one `static class` per view module, with a
/// `params object?[]`-accepting stub per method, returning `""`. The Phase-2
/// model layer's broadcast callbacks call these; Phase 4 replaces the stubs
/// with the real string-builder view renderers.
fn emit_view_stubs(preliminary_views: &[crate::dialect::LibraryClass]) -> Option<EmittedFile> {
    let mut by_module: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for lc in preliminary_views {
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
