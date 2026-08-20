//! Ruby emitter: App → spinel-shape Ruby source files.
//!
//! Mirrors the Crystal emitter's structure: lowered IR (LibraryClass) is
//! the single input, and emission is uniform across Rails components
//! (models, controllers, views, routes, schema all flow through
//! `library::emit_library_class_decl`). No parsed-AST emit path —
//! per the convergence decision, source-equivalence round-trip is no
//! longer a goal; compile-equivalence via Spinel is. Cross-cutting
//! helpers live in `shared`; expression emission lives in `expr`.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::lower::controller::body::FormatBreadth;

/// Canonical spinel test bootstrap. Single source of truth for what
/// the emitted spinel project's `test/test_helper.rb` should contain.
const SPINEL_TEST_HELPER: &str =
    include_str!("../../runtime/spinel/test/test_helper.rb");

pub(crate) mod expr;
mod library;
mod rbs;
pub mod shake;
mod shared;

/// Render a `Ty` to its RBS string form (`String`, `Array[Comment]`,
/// `Article`, `Integer?`). Re-exported for non-emit consumers — e.g. the
/// browser playground's inferred-type hovers (`wasm/src/lib.rs`).
pub use rbs::ty_to_rbs;

// External API: the historical surface kept for `tests/` and `bin/`.
pub use expr::emit_expr;

/// Emit a single `MethodDef` as Ruby source (trailing newline included).
/// The signature and effects are not emitted — they belong to the RBS
/// sidecar, not to Ruby itself. Used by the runtime-extraction pipeline
/// to round-trip a typed standalone function back to Ruby source.
pub fn emit_method(m: &MethodDef) -> String {
    let prefix = match m.receiver {
        MethodReceiver::Instance => "",
        MethodReceiver::Class => "self.",
    };
    let mut ps: Vec<String> = m
        .params
        .iter()
        .map(|p| match (&p.default, p.keyword) {
            (Some(default), false) => {
                format!("{} = {}", p.name.as_str(), emit_default(default))
            }
            (Some(default), true) => {
                format!("{}: {}", p.name.as_str(), emit_default(default))
            }
            (None, true) => format!("{}:", p.name.as_str()),
            (None, false) if p.rest => format!("*{}", p.name.as_str()),
            (None, false) => p.name.as_str().to_string(),
        })
        .collect();
    // The captured block param (`&block`) closes the list — methods that
    // pass their block on (`fetch(&block)`) need it named at the def site.
    if let Some(bp) = &m.block_param {
        ps.push(format!("&{}", bp.name.as_str()));
    }
    let params = if ps.is_empty() {
        String::new()
    } else {
        format!("({})", ps.join(", "))
    };
    let mut out = String::new();
    writeln!(out, "def {prefix}{}{}", m.name, params).unwrap();
    let body_text = emit_expr(&m.body);
    fn emit_default(default: &crate::expr::Expr) -> String {
        let src = expr::emit_expr(default);
        // A command-call default (`raise "…"`, the dead_default
        // grounding) needs parens to close unambiguously inside the
        // parameter list.
        if matches!(&*default.node, crate::expr::ExprNode::Raise { .. }) {
            format!("({src})")
        } else {
            src
        }
    }
    for line in body_text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("end\n");
    out
}

/// Emit library-shape Ruby — for transpiled-shape input where class
/// bodies contain explicit methods rather than Rails DSL calls.
/// Complementary to `emit`; skips Rails-app artifacts (controllers,
/// routes, views, fixtures, importmap, schema) and emits only one
/// `.rb` file per `LibraryClass`. Mirrors `typescript::emit_library`.
pub fn emit_library(app: &App) -> Vec<EmittedFile> {
    library::emit_library_class_decls(app)
}

/// Swap façade-fated extras emits (Sponge) for their raising façades —
/// scaffold-base step; see `library::EXTRAS_FACADES`.
pub fn apply_extras_facades(files: &mut [(String, String)]) {
    library::apply_extras_facades(files)
}

/// Restore the verbatim source-shape emit over the extras façades
/// (CRuby tree — the real stdlib is available there).
pub fn restore_extras_facades(files: &mut [(String, String)], app: &App) {
    library::restore_extras_facades(files, app)
}

/// Per-app `Rails::Application` reopen from the source
/// config/application.rb (see `App.rails_application`) — emitted to
/// `config/application.rb` so `Rails.application.read_only?` etc.
/// dispatch to the app's real config methods. The scaffold main.rb
/// requires it conditionally (same idiom as config/importmap.rb).
pub fn emit_rails_application(app: &App) -> Option<EmittedFile> {
    app.rails_application.as_ref().map(|lc| {
        let mut lc = lc.clone();
        // Ground `Rails.application.routes.url_helpers.<x>_url(host:,
        // protocol:)` (lobsters' root_url) — the routing internals
        // behind url_helpers aren't modeled, but the absolute URL is
        // just protocol + "://" + host + RouteHelpers.<x>_path.
        for m in &mut lc.methods {
            library::rewrite_url_helpers_absolute(&mut m.body);
        }
        library::emit_library_class_decl(&lc, app, PathBuf::from("config/application.rb"))
    })
}

/// Lower each `app.models` entry through `model_to_library` and emit
/// the resulting `LibraryClass` as a Ruby source file. The output is
/// the universal post-lowering shape — explicit per-attr accessors,
/// explicit `validate` / `before_destroy` bodies, no Rails DSL.
///
/// Spinel is the natural validation target for the lowering pipeline:
/// the lowered IR shape *is* spinel-blog shape (per
/// `project_universal_post_lowering_ir.md`), so a Ruby render is the
/// shortest path from lowerer output to a runnable artifact. Use this
/// while accumulating lowerers; the per-target collapse decisions for
/// TS / Rust / etc. are deferred until enough lowerers exist for
/// natural groupings to surface.
/// Retype a model's synthesized `from_stmt(stmt)` class-method param to
/// `Untyped`. See the call site in `emit_lowered_models` for why this is
/// a ruby/spinel-only adapter (the stmt handle is a `void *` here, an
/// integer cursor on the strict targets).
fn relax_from_stmt_handle(lc: &mut LibraryClass) {
    for m in &mut lc.methods {
        if m.name.as_str() == "from_stmt" && m.receiver == MethodReceiver::Class {
            if let Some(crate::ty::Ty::Fn { params, .. }) = m.signature.as_mut() {
                if let Some(p) = params.first_mut() {
                    p.ty = crate::ty::Ty::Untyped;
                }
            }
        }
    }
}

pub fn emit_lowered_models(app: &App) -> Vec<EmittedFile> {
    // Collect controller `permit(...)` declarations so the model lowerer
    // can synthesize `from_params(p: <Resource>Params)` factories sized
    // to the permitted-fields list. See `controller_to_library/params.rs`.
    let params_specs =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);

    // Bulk lower so per-resource synthesized siblings (`<Model>Row`)
    // ride alongside the model class. Each returned `LibraryClass`
    // becomes one `app/models/<stem>.rb` file. *Params classes are
    // synthesized by the controller lowerer (separate emit path —
    // `emit_lowered_controllers`); we register them here as
    // synthesized siblings so model files that reference them
    // (`Article.from_params(...)` calls) get explicit requires.
    // Class methods a call site reaches through an association and whose
    // body QUERIES — `@room.messages.paged?`. `apply_scope_lowering`
    // below re-roots those on a threaded `__rel`, so the arel fold has
    // to be held back on them here or there is nothing left to re-root.
    // Same survey both passes read, for the same reason the survey
    // covers the App rather than a slice.
    let assoc_scopes = crate::lower::scope_chain::assoc_query_method_names(
        &crate::lower::scope_chain::survey_assoc_class_methods(
            app,
            &crate::lower::scope_chain::build_assoc_registry(&app.models),
            &crate::lower::scope_chain::build_scope_registry(&app.models),
        )
        .0,
    );
    let mut lcs = crate::lower::model_to_library::lower_models_to_library_classes_unfolding(
        &app.models,
        &app.schema,
        Vec::new(),
        &params_specs,
        &assoc_scopes,
    );
    // The sqlite statement handle `Db.prepare` returns is a per-target
    // `Db` primitive: an integer cursor on most adapters (the shared
    // model lowerer's `Ty::Int` default), but an opaque FFI `void *` on
    // the spinel shim (`runtime/spinel/db.rb`). Relax the synthesized
    // `from_stmt(stmt)` param to `untyped` so the emitted `.rbs` doesn't
    // pin it to `Integer` — spinel infers the pointer from the
    // `Db.column_*(stmt, …)` FFI calls, and CRuby ignores the sig
    // entirely. Confined here, the only emitter whose `Db` hands back a
    // raw pointer; the strict targets keep `Ty::Int` (correct for their
    // integer-handle `Db`). See the toolchain-spinel `from_stmt` seam.
    for lc in &mut lcs {
        relax_from_stmt_handle(lc);
    }

    // Ruby-family scope lowering: synthesize model scope methods +
    // normalize scope chains before rendering (no-op for scope-free apps).
    library::apply_scope_lowering(&mut lcs, app);
    // has_many :through readers: rebuild the shared direct-fk reader as a
    // Relation join through the intermediate table (no-op when no
    // through-assoc resolves).
    library::apply_through_assoc_lowering(&mut lcs, app);
    // belongs_to writers are synthesized by the shared model lowering
    // (lower::model_to_library::associations), alongside the readers.
    // App-helper resolution: bare `avatar_img(...)` → `ApplicationHelper.
    // avatar_img(...)` + helper modules become module-functions (no-op when
    // the app ships no non-empty helpers).
    library::apply_helper_lowering(&mut lcs, app);
    // A record handed to a path helper becomes its slug here, not
    // inside the helper (no-op unless a model overrides `to_param`).
    library::apply_route_param_lowering(&mut lcs, app);
    // A helper handed `raw(x)` retargets to a `_raw` clone in which
    // that argument's escaping comes off (no-op without such a site).
    library::apply_raw_helper_monomorphization(&mut lcs, app);
    // Dynamic `send(k)` → static `case` dispatch moved to the shared
    // post-analyze hook (`lower::apply_send_static_dispatch`) — hook
    // bodies arrive here already grounded, with duration-unit arms in
    // plural form for the duration lowering below.
    // `record.update!(k: v)` inlining moved to the shared post-analyze
    // hook (`lower::apply_update_kwargs_inline`) — hook bodies arrive
    // here already in writer-assign + save form.
    // `Time.current` and ActiveSupport durations (`70.days` →
    // `Duration.days(70)`) are grounded app-wide by the shared
    // post-analyze hook (`lower::apply_time_current_lowering`,
    // `lower::apply_duration_lowering`); only the view pipeline still
    // applies the emit-time copies.
    // Date/DateTime/Time column accessors coerce to/from real `Time`
    // (no-op for apps with no temporal columns).
    library::apply_datetime_lowering(&mut lcs, app);
    // has_secure_password (authenticate + plaintext accessors, bcrypt
    // contract) and typed_store virtual-attribute accessors are
    // synthesized by the shared model lowering
    // (`lower::secure_password`, `lower::typed_store`) — they arrive
    // here already present.
    // Boolean-column readers/predicates cast SQLite's 0/1 Integers
    // (0 is truthy in Ruby).
    library::apply_boolean_lowering(&mut lcs, app);
    // NULL fidelity: nullable columns hydrate to nil (not 0/"") and fk
    // 0-sentinel guards widen to accept nil (no-op for schemas with no
    // nullable columns); synthesized `.empty?` predicate forms in
    // model bodies tolerate the nil.
    library::apply_hydration_nil_lowering(&mut lcs, app);
    library::apply_nilsafe_empty_lowering(&mut lcs);
    library::apply_time_format_lowering(&mut lcs);
    library::apply_dynamic_render_options_lowering(&mut lcs);
    // Runtime-Relation eager loading: per-model `preload_associations`
    // machinery + belongs_to reader cache guards, so `includes(...)`
    // on a runtime Relation executes as batched IN-loads instead of
    // N+1 lazy reads. Runs last so nothing reprocesses the synthesized
    // bodies (no-op unless the app has scopes AND surviving includes).
    library::apply_preload_lowering(&mut lcs, app);

    // Synthesized siblings need explicit `require_relative` even when
    // they live in the same directory as their referencer — nothing else
    // in the require chain loads them. Build a (name, anchor) map from
    // every LC carrying an `origin` tag, plus the *Params classes that
    // controllers will synthesize separately.
    let mut synthesized: Vec<(String, String)> = lcs
        .iter()
        .filter(|lc| lc.origin.is_some())
        .map(|lc| {
            let name = lc.name.0.as_str().to_string();
            let stem = crate::naming::underscore(&name);
            (name, format!("app/models/{stem}"))
        })
        .collect();
    for spec in params_specs.iter() {
        let name = spec.class_id.0.as_str().to_string();
        let stem = crate::naming::underscore(&name);
        synthesized.push((name, format!("app/models/{stem}")));
    }

    lcs.iter()
        .flat_map(|lc| {
            let stem = crate::naming::underscore(lc.name.0.as_str());
            let out_path = PathBuf::from(format!("app/models/{stem}.rb"));
            library::emit_library_class_pair_with_synthesized(
                lc,
                app,
                out_path,
                &synthesized,
            )
        })
        .collect()
}

/// Emit `config/schema.rb` in spinel-blog shape — a `Schema` module
/// with `def self.statements` returning the DDL list. Per-statement
/// (rather than one joined string) so adapters that don't support
/// multi-statement execution work too. Consumes the universal
/// `lower_schema_to_library_functions` output, sharing shape across
/// every target.
pub fn emit_lowered_schema(app: &App) -> EmittedFile {
    let funcs = crate::lower::lower_schema_to_library_functions(&app.schema);
    library::emit_module_file(&funcs, app, PathBuf::from("config/schema.rb"))
}

/// Pair variant of `emit_lowered_schema` — produces both `.rb` and
/// `.rbs`. Replaces `emit_lowered_schema` at call sites that want
/// the typed sidecar emitted alongside.
pub fn emit_lowered_schema_pair(app: &App) -> Vec<EmittedFile> {
    let funcs = crate::lower::lower_schema_to_library_functions(&app.schema);
    library::emit_module_file_pair(&funcs, app, PathBuf::from("config/schema.rb"))
}

/// Emit `config/routes.rb` in spinel-blog shape — a `RouteTable` module
/// `RouteTable` module exposing the dispatch data via class methods:
/// `RouteTable.table` returns the array of `{method:, pattern:,
/// controller:, action:}` hashes; `RouteTable.root` returns the
/// shorthand `root "c#a"` route (when present). Companion to
/// `emit_lowered_models` and `emit_lowered_schema` for the spinel
/// emit pipeline.
///
/// Method-form (rather than `Routes::TABLE` constant) shares shape
/// with the universal LibraryFunction emit consumed by every other
/// target. Same data shape as Importmap.pins / Schema.statements.
///
/// A small controller-requires header lives at the top of the file
/// because the Spinel runtime expects per-controller files to be
/// loaded by side effect when `config/routes.rb` is required from
/// `main.rb`. The body itself (the data) flows through the
/// universal walker.
pub fn emit_lowered_routes(app: &App) -> EmittedFile {
    let funcs = crate::lower::lower_routes_to_dispatch_functions(app);
    let mut emitted = library::emit_module_file(
        &funcs,
        app,
        PathBuf::from("config/routes.rb"),
    );

    // Prepend require_relative headers for application_controller and
    // each unique controller used by the route table — Spinel runtime
    // loads controllers via require chain rooted at config/routes.rb.
    let flat = crate::lower::routes::flatten_routes(app);
    let mut header = String::new();
    use std::fmt::Write;
    writeln!(
        header,
        "require_relative \"../app/controllers/application_controller\""
    )
    .unwrap();
    let mut seen: Vec<String> = vec!["application_controller".to_string()];
    // Only controllers the app actually DEFINES. Rails resolves a
    // controller lazily at dispatch, so a route naming one that does not
    // exist raises only when that path is requested; campfire routes
    // `resource :settings` inside `scope module: "rooms"` and ships no
    // `Rooms::SettingsController` at all. Requiring it eagerly turned
    // that dangling route into a hard failure for the WHOLE app — a
    // LoadError at boot on the ruby lane and a build abort on spinel.
    // Skipping it keeps Rails' laziness: the route still dispatches,
    // and it fails where Rails fails it.
    let defined: std::collections::HashSet<&str> =
        app.controllers.iter().map(|c| c.name.0.as_str()).collect();
    for r in &flat {
        let class_name = r.controller.0.as_str();
        if !defined.contains(class_name) {
            continue;
        }
        let stem = crate::naming::underscore(class_name);
        if seen.contains(&stem) {
            continue;
        }
        seen.push(stem.clone());
        writeln!(header, "require_relative \"../app/controllers/{stem}\"").unwrap();
    }
    writeln!(header).unwrap();
    emitted.content = format!("{header}{}", emitted.content);
    emitted
}

/// Emit each controller in spinel-blog shape: a `process_action(action_name)`
/// dispatcher (synthesizing before-action filters as conditional calls
/// and case-dispatching to per-action methods) plus the public actions
/// and private filter targets as ordinary methods. Output is one
/// `app/controllers/<name>.rb` per non-synthesized class; tagged
/// synthesized siblings (`<Resource>Params` holders) route to
/// `app/models/<name>.rb` because they're plain holders, not request
/// handlers.
pub fn emit_lowered_controllers(app: &App) -> Vec<EmittedFile> {
    // RSS_ONLY: the spinel tree compiles these, so the inline-json arms
    // stay out — they reference the CRuby-overlay JsonRender and don't
    // type under the AOT compile (CI caught `render(JsonRender.encode
    // (...))` as sp_RbVal-vs-char* in articles_controller). The rss arm
    // reaches only the emitted `Views::<X>.rss` template and the shared
    // `Rails.cache.fetch_str`, both of which this tree already carries.
    let mut lcs = lower_controllers_for_spinel(app, FormatBreadth::RSS_ONLY);
    library::apply_scope_lowering(&mut lcs, app);
    library::apply_helper_lowering(&mut lcs, app);
    // A record handed to a path helper becomes its slug here, not
    // inside the helper (no-op unless a model overrides `to_param`).
    library::apply_route_param_lowering(&mut lcs, app);
    // A helper handed `raw(x)` retargets to a `_raw` clone in which
    // that argument's escaping comes off (no-op without such a site).
    library::apply_raw_helper_monomorphization(&mut lcs, app);
    library::apply_nilsafe_empty_lowering(&mut lcs);
    library::apply_time_format_lowering(&mut lcs);
    library::apply_dynamic_render_options_lowering(&mut lcs);
    // Layout wrap at render call sites — the seam where the @ivars a
    // layout reads are in scope. Converged with the CRuby tree
    // (emit_lowered_controllers_with_layout below): the scaffold
    // dispatch ships `controller.body` verbatim on every tree, and a
    // multi-param layout (lobsters takes its 12-14 ivar-derived
    // params) can only be called where those ivars live. No-op for
    // apps without an emitted Layouts/application view.
    library::apply_layout_lowering(&mut lcs, app);
    emit_lowered_controllers_from_lcs(&lcs, app)
}

/// CRuby/JRuby-tree variant: controllers with the layout wrap —
/// `render(Views::X.y(...))` → `render(Views::Layouts.application(
/// Views::X.y(...), @<ivar>…, @flash…))` — so a layout reading
/// controller ivars receives them. Pairs with the ruby_overlay main.rb
/// shipping `controller.body` verbatim. MUST NOT run on the plain
/// spinel target: its dispatch still wraps body-only, and wrapping in
/// both places renders the layout twice (the second pass reads
/// already-consumed content_for slots — CI caught the double wrap as
/// nil crashes in the spinel compare). The ruby/jruby project layers
/// re-emit controllers through this and dedupe last-wins over the
/// spinel-shape files.
pub fn emit_lowered_controllers_with_layout(app: &App) -> Vec<EmittedFile> {
    // FULL: the CRuby/JRuby trees ship the overlay (JsonRender) so the
    // widened respond_to arms all resolve; these files dedupe last-wins
    // over the spinel-shape ones.
    let mut lcs = lower_controllers_for_spinel(app, FormatBreadth::FULL);
    library::apply_scope_lowering(&mut lcs, app);
    library::apply_helper_lowering(&mut lcs, app);
    // A record handed to a path helper becomes its slug here, not
    // inside the helper (no-op unless a model overrides `to_param`).
    library::apply_route_param_lowering(&mut lcs, app);
    // A helper handed `raw(x)` retargets to a `_raw` clone in which
    // that argument's escaping comes off (no-op without such a site).
    library::apply_raw_helper_monomorphization(&mut lcs, app);
    library::apply_nilsafe_empty_lowering(&mut lcs);
    library::apply_time_format_lowering(&mut lcs);
    library::apply_dynamic_render_options_lowering(&mut lcs);
    library::apply_layout_lowering(&mut lcs, app);
    emit_lowered_controllers_from_lcs(&lcs, app)
}

/// Bulk lower controllers in spinel-shape. Synthesized siblings
/// (`<Resource>Params`) ride alongside the controller classes in the
/// returned vec.
///
/// Lowers models first so the model `ClassInfo`s land in the
/// controller lowerer's registry as extras — the Arel pass needs
/// them to resolve `Article.includes(...).order(...)` chain
/// receivers to a TableRef.
fn lower_controllers_for_spinel(app: &App, format_breadth: FormatBreadth) -> Vec<LibraryClass> {
    // Use lower_models_with_registry (not lower_models_to_library_classes
    // + class_info_from_library_class) because the former returns
    // ClassInfo with `table` set — the Arel pass needs `info.table`
    // to map a Const recv to a TableRef when recognizing chains.
    let (_, model_registry) = crate::lower::lower_models_with_registry(
        &app.models,
        &app.schema,
        Vec::new(),
    );
    let model_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    // Association graph drives `includes(:assoc)` eager-load lowering
    // (issue #27) — without it the Arel pass drops includes and the
    // reader N+1s.
    let assocs = crate::lower::model_associations::compute_association_graph(app);
    // Route-reachability per controller: a public method is a routable
    // action (gets implicit render + process_action dispatch) only if a
    // route reaches it. Lets base-controller helper/filter methods keep
    // their return value instead of being clobbered by a synthesized render.
    let mut routed: std::collections::HashMap<
        crate::ident::ClassId,
        std::collections::HashSet<crate::ident::Symbol>,
    > = std::collections::HashMap::new();
    for r in crate::lower::flatten_routes(app) {
        routed.entry(r.controller).or_default().insert(r.action);
    }
    let route_ids = crate::lower::routes::helper_id_segments(app);
    crate::lower::lower_controllers_with_arel_views_assocs_and_routes(
        &app.controllers,
        model_extras,
        crate::lower::controller_to_library::LowerControllerOptions {
            schema: Some(&app.schema),
            views: &app.views,
            library_classes: &app.library_classes,
            assocs: &assocs,
            routed_by_controller: Some(&routed),
            format_breadth,
            route_id_segments: Some(&route_ids),
        },
    )
}

/// Render pre-lowered controller `LibraryClass`es to one
/// `app/controllers/<stem>.rb` per non-synthesized class plus
/// `app/models/<stem>.rb` for tagged synthesized siblings.
fn emit_lowered_controllers_from_lcs(
    lcs: &[LibraryClass],
    app: &App,
) -> Vec<EmittedFile> {
    let synthesized: Vec<(String, String)> = lcs
        .iter()
        .filter(|lc| lc.origin.is_some())
        .map(|lc| {
            let name = lc.name.0.as_str().to_string();
            let stem = crate::naming::underscore(&name);
            (name, format!("app/models/{stem}"))
        })
        .collect();

    lcs.iter()
        .flat_map(|lc| {
            let file_stem = crate::naming::underscore(lc.name.0.as_str());
            let out_path = if lc.origin.is_some() {
                PathBuf::from(format!("app/models/{file_stem}.rb"))
            } else {
                PathBuf::from(format!("app/controllers/{file_stem}.rb"))
            };
            let mut files = library::emit_library_class_pair_with_synthesized(
                lc,
                app,
                out_path,
                &synthesized,
            );
            // Sibling error-class declarations captured from the
            // controller's source file (`class LoginFailedError <
            // StandardError; end` before `class LoginController`) —
            // re-declared ahead of the controller class so the
            // actions' raise/rescue sites resolve.
            if lc.origin.is_none() {
                if let Some(ctrl) =
                    app.controllers.iter().find(|c| c.name == lc.name)
                {
                    if !ctrl.sibling_classes.is_empty() {
                        for f in files.iter_mut() {
                            if f.path.extension().is_some_and(|e| e == "rb") {
                                prepend_sibling_classes(
                                    &mut f.content,
                                    &ctrl.sibling_classes,
                                    lc.name.0.as_str(),
                                );
                            }
                        }
                    }
                }
            }
            files
        })
        .collect()
}

/// Insert `class <Name> < <Parent>; end` declaration lines directly
/// above the controller's own `class` line (mirroring where the source
/// file put them). No-op when the class line isn't found.
fn prepend_sibling_classes(
    content: &mut String,
    siblings: &[(crate::ident::Symbol, crate::ident::Symbol)],
    class_name: &str,
) {
    let marker = format!("class {class_name}");
    let Some(pos) = content.find(&marker) else { return };
    let mut decls = String::new();
    for (name, parent) in siblings {
        decls.push_str(&format!(
            "class {} < {}; end\n",
            name.as_str(),
            parent.as_str()
        ));
    }
    decls.push('\n');
    content.insert_str(pos, &decls);
}

/// Lower each html-format `app.views` entry through `view_to_library`
/// and emit the resulting `LibraryClass` as a Ruby source file under
/// `app/views/<dir>/<base>.rb`. Output is the universal post-lowering
/// shape: a `Views::<Plural>` module with one `def self.<action>(args)`
/// per view, body in `io = String.new ; io << ViewHelpers.x(...) ; io`
/// form. See `project_universal_post_lowering_ir.md`.
///
/// json-format views (`*.json.jbuilder`) go through
/// `emit_lowered_jbuilder_views` — same shape, separate file per
/// template named `<base>_json.rb` to avoid colliding with the html
/// sibling. Both files reopen the same `Views::<Plural>` module.
pub fn emit_lowered_views(app: &App) -> Vec<EmittedFile> {
    // Lower every view through ONE per-app context, then run the
    // Ruby-family passes over the whole group at once — per-view pass
    // application rebuilt the scope/assoc/helper registries for every
    // template, which is quadratic in app size (the mastodon playground
    // transpile blew its budget on it).
    let vctx = crate::lower::ViewLowerCtx::new(app);
    let html_views: Vec<&crate::dialect::View> = app
        .views
        .iter()
        .filter(|v| crate::lower::view::renders_through_view_path(v.format.as_str()))
        .collect();
    let mut lcs: Vec<LibraryClass> = html_views.iter().map(|v| vctx.lower(v)).collect();
    // Normalize scope chains opened in the template itself — lobsters'
    // _listdetail runs `story.merged_stories.not_deleted.includes(...)`
    // — then resolve bare app-helper calls (`avatar_img(...)` →
    // `ApplicationHelper.avatar_img(...)`). Scope lowering is a strict
    // no-op for scope-free apps (the blog), and a view LC is never a
    // model, so only the call-site rewrite arm runs.
    library::apply_scope_lowering(&mut lcs, app);
    library::apply_helper_lowering(&mut lcs, app);
    // A record handed to a path helper becomes its slug here, not
    // inside the helper (no-op unless a model overrides `to_param`).
    library::apply_route_param_lowering(&mut lcs, app);
    // A helper handed `raw(x)` retargets to a `_raw` clone in which
    // that argument's escaping comes off (no-op without such a site).
    library::apply_raw_helper_monomorphization(&mut lcs, app);
    library::apply_time_current_lowering(&mut lcs, app);
    library::apply_duration_lowering(&mut lcs, app);
    // Nullable columns hydrate to nil on the Ruby tree — synthesized
    // `.empty?` predicate forms in view bodies must tolerate it.
    library::apply_nilsafe_empty_lowering(&mut lcs);
    library::apply_time_format_lowering(&mut lcs);
    library::apply_dynamic_render_options_lowering(&mut lcs);
    // A view whose own namespace shadows a top-level app class
    // (`Views::Stats` over `Stats`) must reference it absolutely, or
    // Ruby resolves the inner module first. No-op unless a view
    // directory shares a name with a class.
    library::apply_constant_rooting(&mut lcs, app, library::RootingScope::AppAndRuntime);
    html_views
        .iter()
        .zip(lcs.iter())
        .flat_map(|(v, lc)| {
            let out_path = view_output_path(v.name.as_str());
            library::emit_library_class_pair(lc, app, out_path)
        })
        .collect()
}

/// Lower each json-format `app.views` entry through `jbuilder_to_library`
/// and emit the resulting `LibraryClass` as a Ruby source file under
/// `app/views/<dir>/<base>_json.rb`. Method body uses the same
/// io-accumulator shape as html views; values flow through
/// `JsonBuilder.encode_value`.
pub fn emit_lowered_jbuilder_views(app: &App) -> Vec<EmittedFile> {
    app.views
        .iter()
        .filter(|v| v.format.as_str() == "json")
        .flat_map(|v| {
            let lc = crate::lower::lower_jbuilder_to_library_class(v, app);
            let out_path = jbuilder_view_output_path(v.name.as_str());
            library::emit_library_class_pair(&lc, app, out_path)
        })
        .collect()
}

/// Map a view name (`articles/index`, `articles/_article`,
/// `layouts/application`) to the output path under `app/views/`.
/// Partials retain their leading underscore in the basename so the
/// require-relative graph keeps working without a separate alias step.
fn view_output_path(view_name: &str) -> PathBuf {
    PathBuf::from(format!("app/views/{view_name}.rb"))
}

/// Jbuilder counterpart of `view_output_path`. The `_json` suffix
/// matches the lowered method name and prevents path collision with
/// the html sibling: `articles/_article.json.jbuilder` →
/// `app/views/articles/_article_json.rb`.
fn jbuilder_view_output_path(view_name: &str) -> PathBuf {
    PathBuf::from(format!("app/views/{view_name}_json.rb"))
}

/// Spinel-shape emit: lowered IR rendered as runnable Ruby. Composes
/// the five `emit_lowered_*` functions into a single project — schema,
/// routes, models, controllers, views — laid out under the spinel
/// target's directory shape (app/, config/, test/). The natural
/// validation target of the lowering pipeline: CRuby executes the
/// output until spinel grows its own test runner.
///
/// Every emitted test file ends with an explicit per-test driver
/// shim (`render_autorun_shim`). Mirrors the role Crystal's
/// `macro inherited` plays at Crystal compile time and TS's
/// `discover_tests(klass)` plays at JS load time. The shim works
/// under both CRuby and spinel-AOT — same file, same mechanism, no
/// reliance on Minitest's at_exit autorun (which spinel can't see and
/// which would have CRuby double-run every test if combined with the
/// shim).
pub fn emit_spinel(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.extend(emit_lowered_schema_pair(app));
    // routes.rb gets a require-relative header prepended (see
    // emit_lowered_routes); emit the .rb via that path, then derive
    // the .rbs sidecar from the same funcs without the header.
    let route_funcs = crate::lower::lower_routes_to_dispatch_functions(app);
    files.push(emit_lowered_routes(app));
    files.extend(rbs_only_from_funcs(
        &route_funcs,
        PathBuf::from("config/routes.rb"),
    ));
    let importmap_funcs = crate::lower::lower_importmap_to_library_functions(app);
    if !importmap_funcs.is_empty() {
        files.extend(library::emit_module_file_pair(
            &importmap_funcs,
            app,
            PathBuf::from("config/importmap.rb"),
        ));
    } else {
        // The scaffold main.rb `require_relative`s this file
        // unconditionally — spinel's static require graph has no
        // begin/rescue escape hatch (the CRuby overlay's conditional
        // require is exactly that hatch). A source app without an
        // importmap still needs the file to exist; a bare reopen keeps
        // the `runtime/importmap.rb` fallback's pins/entry standing.
        files.push(EmittedFile {
            path: PathBuf::from("config/importmap.rb"),
            content: "# No importmap in the source app; the runtime/importmap\n\
                      # fallback's empty pins stand. This file exists because the\n\
                      # scaffold main.rb requires it unconditionally (spinel AOT\n\
                      # resolves the whole require graph statically).\n\
                      module Importmap\n\
                      end\n"
                .to_string(),
        });
    }

    // Per-app Rails::Application reopen (config/application.rb) — the
    // app's real config methods (`read_only?`, `name`, `domain`) so
    // `Rails.application.<m>` dispatches to them instead of the empty
    // runtime shim. Same unconditional-require contract as importmap:
    // apps without one get a bare reopen stub.
    match emit_rails_application(app) {
        Some(f) => files.push(f),
        None => files.push(EmittedFile {
            path: PathBuf::from("config/application.rb"),
            content: "# No config/application.rb methods in the source app; the\n\
                      # runtime's empty Rails::Application shim stands. This file\n\
                      # exists because the scaffold main.rb requires it\n\
                      # unconditionally (spinel AOT resolves the whole require\n\
                      # graph statically).\n\
                      module Rails\n\
                      \x20\x20class Application\n\
                      \x20\x20end\n\
                      end\n"
                .to_string(),
        }),
    }
    files.extend(emit_lowered_models(app));
    if let Some(f) = library::emit_relation_scope_delegates(app) {
        files.push(f);
    }
    files.extend(emit_lowered_controllers(app));
    files.extend(emit_lowered_views(app));
    files.extend(emit_lowered_jbuilder_views(app));

    // RouteHelpers — `app/route_helpers.rb` with `def self.<x>_path(args)`
    // per named route. Generated from `app.routes`; supersedes the
    // hand-written `runtime/ruby/action_view/route_helpers.rb` (which
    // is being kept for backward compat until callers migrate).
    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    // The segment interpolations stay bare. `to_param` used to be
    // applied here, to every segment of every helper — which reads as
    // Rails-faithful but isn't, because our segments are declared
    // `String` and Rails' are untyped: under AOT the record was already
    // coerced away by the time the body ran. It now happens at the call
    // site, where the record still is one
    // (`library::apply_route_param_lowering`).
    if !route_helper_funcs.is_empty() {
        files.extend(library::emit_module_file_pair(
            &route_helper_funcs,
            app,
            PathBuf::from("app/route_helpers.rb"),
        ));
    }

    // Seeds — `db/seeds.rb` as a `Seeds.run` module method. Mirrors
    // the TS pipeline; was previously missing from spinel emit.
    let seeds_funcs = crate::lower::lower_seeds_to_library_functions(app);
    if !seeds_funcs.is_empty() {
        files.extend(library::emit_module_file_pair(
            &seeds_funcs,
            app,
            PathBuf::from("db/seeds.rb"),
        ));
    }

    // Lower fixtures up-front so the test_helper renderer can list them
    // explicitly (replacing the source-side `Object.constants.sort.each`
    // + `Object.const_get` scan, which violates the spinel subset).
    // Same `fixture_lcs` is reused below for the per-fixture file emit
    // and for `fixture_extras` synthesized siblings.
    let mut fixture_lcs = crate::lower::lower_fixtures_to_library_classes(app);
    // Same vestige as the view pipeline above, for the same reason:
    // fixture classes are synthesized here, after the shared post-
    // analyze hooks have already run over the App, so an ERB tag's
    // `<%= 1.hour.ago %>` reaches emit ungrounded unless it takes the
    // rewrite on the way out. No-op for fixtures without ERB.
    library::apply_duration_lowering(&mut fixture_lcs, app);

    // Per-test truncation, shared by `SchemaSetup.reset!` in the test
    // helper and by each test file's autorun shim below. ONE list: the
    // two disagreeing was the bug — the shim truncated correctly while
    // the helper's stale `Article`/`Comment` pair no-opped, so
    // `reset!` re-loaded every fixture onto rows it had not removed
    // (campfire: `UNIQUE constraint failed: accounts.singleton_guard`).
    //
    // Only models with a backing schema table get an
    // `_adapter_truncate` (synthesized by push_adapter_methods under
    // the same gate). Abstract parents like ApplicationRecord have no
    // table → no truncate method → calling it falls through to
    // ActiveRecord::Base and raises NotImplementedError. Skip them.
    let mut truncate_lines: Vec<String> = app
        .models
        .iter()
        .filter(|m| app.schema.tables.contains_key(&m.table.0))
        .map(|m| format!("{}._adapter_truncate", m.name.0.as_str()))
        .collect();
    // A table with NO model still holds rows a test wrote, and
    // reloading fixtures on top of them is the same bug the shared list
    // above exists to prevent. campfire's `message_search_index` is the
    // case: an fts5 virtual table the app keeps in step by hand
    // (`Message::Searchable`), so every message insert added a row that
    // nothing cleared, and the second fixture load hit `constraint
    // failed` on a rowid it had already used.
    //
    // `DELETE FROM` rather than a synthesized `_adapter_truncate`:
    // there is no model to synthesize it onto, and a virtual table has
    // no AUTOINCREMENT sequence to reset.
    let modelled: std::collections::HashSet<&str> =
        app.models.iter().map(|m| m.table.0.as_str()).collect();
    for (name, _table) in &app.schema.tables {
        if !modelled.contains(name.as_str()) {
            truncate_lines.push(format!("Db.exec(\"DELETE FROM {}\")", name.as_str()));
        }
    }

    // Test bootstrap. The canonical content (LOAD_PATH wiring,
    // SqliteAdapter setup, RequestDispatch + ActionResponse +
    // SchemaSetup modules) lives at `runtime/spinel/test/test_helper.rb`
    // so the standalone fixture and overlay flows share one source.
    // The renderer substitutes the three per-app pieces the source
    // carries blog-shaped stand-ins for — see `render_test_helper`.
    // Emitted unconditionally — every spinel project carries the helper
    // even when no test files are produced yet.
    files.push(EmittedFile {
        path: PathBuf::from("test/test_helper.rb"),
        content: render_test_helper(&fixture_lcs, &truncate_lines),
    });

    // Test fixtures — one `<Plural>Fixtures` LibraryClass per YAML file
    // under `test/fixtures/`, rendered to `test/fixtures/<plural>.rb`.
    // Mirrors the TS pattern at `typescript.rs:302-306`. Available for
    // emitted tests to consume via `ArticlesFixtures.one()` (the call
    // shape `lower_test_modules_to_library_classes` rewrites
    // `articles(:one)` to).
    for lc in &fixture_lcs {
        let stem = fixture_file_stem(lc.name.0.as_str());
        let out_path = PathBuf::from(format!("test/fixtures/{stem}.rb"));
        files.extend(library::emit_library_class_pair(lc, app, out_path));
    }

    // Test modules — lower each `XTest` class into a `LibraryClass`
    // whose methods are `def test_<snake>` blocks (one per `test "..."`
    // macro), then render to `test/models/<stem>_test.rb` or
    // `test/controllers/<stem>_test.rb` depending on the class name
    // suffix. Mirrors `typescript.rs:308-325`. Empty extras for now
    // (the lowerer registers minitest baseline + framework stubs +
    // fixture helpers internally); broader extras assembly can land
    // when a test body needs more than the lowerer's own registry.
    if !app.test_modules.is_empty() {
        // Each `<Plural>Fixtures` LibraryClass surfaces its label
        // methods (typed `() -> Class<Model>`) and `_fixtures_load!`
        // through the registry so test bodies that bind a local from
        // `ArticlesFixtures.one` get the parent's class type — which
        // is what the has-many `.create`/`.build` rewrite consults to
        // de-magic `article.comments.create(...)` into
        // `Comment.create(article_id: article.id, ...)`.
        // App models ride along beside the fixture classes. A test body
        // names them constantly — `users(:david).rooms.last` — and with
        // no entry the has_many reader resolves to nothing, so the
        // record never types and every type-directed rewrite downstream
        // (the route-helper id projection among them) declines.
        //
        // Inside a diagnostic scope, and the collected diagnostics are
        // DROPPED: this is a second call to the model lowerer purely for
        // its registry, and letting it report again duplicated 58
        // `unsupported` entries into campfire's ledger — the same gaps,
        // counted twice, which is worse than not knowing.
        let (model_registry, _dup_diags) = crate::emit::diagnostics::scope(|| {
            let (_, reg) =
                crate::lower::lower_models_with_registry(&app.models, &app.schema, Vec::new());
            reg
        });
        let fixture_extras: Vec<(crate::ident::ClassId, crate::analyze::ClassInfo)> = fixture_lcs
            .iter()
            .map(|lc| (lc.name.clone(), crate::lower::class_info_from_library_class(lc)))
            .chain(model_registry)
            .collect();
        let mut test_lowered = crate::lower::lower_test_modules_with_inner(
            &app.test_modules,
            &app.fixtures,
            &app.models,
            fixture_extras,
            &crate::lower::routes::helper_id_segments(app),
        );
        // A test body writes the same relation chains an app body does —
        // `@room.messages.ordered`, `user.push_subscriptions.find_by(…)`,
        // `room.memberships.grant_to(users)`. The has_many reader hands
        // back the arel-inlined Array, so every one of those needs the
        // association-seeded Relation that `apply_scope_lowering` builds
        // (`Relation.new(Message).where(room_id: @room.id).ordered`). It
        // runs over models, controllers, helpers and views; the test
        // classes were the one lowered family it never saw, so the exact
        // chain that compiled inside a controller raised NoMethodError
        // one file over.
        //
        // This is the same drift wall 21 found on the REGISTRY side (test
        // bodies typed against a registry with no scopes in it). Same two
        // sides, same fix: one seam, both callers.
        //
        // Inner classes ride along — a `class Validatable` declared in a
        // test body is an ordinary lowered class and may query too. No
        // entry in `app.models` matches a test class, so the model-only
        // half (scope-method synthesis, `push_scope_variants`) is inert
        // here and only the call-site rewrite runs.
        {
            let mut test_lcs: Vec<crate::dialect::LibraryClass> = test_lowered
                .iter()
                .flat_map(|m| {
                    std::iter::once(m.test_class.clone()).chain(m.inner_classes.iter().cloned())
                })
                .collect();
            library::apply_scope_lowering(&mut test_lcs, app);
            let mut it = test_lcs.into_iter();
            for m in &mut test_lowered {
                m.test_class = it.next().expect("one lowered class per test module");
                for inner in &mut m.inner_classes {
                    *inner = it.next().expect("one lowered class per inner class");
                }
            }
        }
        // Fixture classes (`ArticlesFixtures`, etc.) live at
        // `test/fixtures/<plural>.rb` — outside the model/controller
        // require-resolution paths the library emitter knows. Pass them
        // as synthesized siblings so any test body that references one
        // gets an explicit `require_relative "../fixtures/<plural>"`.
        let fixture_siblings: Vec<(String, String)> = fixture_lcs
            .iter()
            .map(|lc| {
                let name = lc.name.0.as_str().to_string();
                let stem = fixture_file_stem(&name);
                (name, format!("test/fixtures/{stem}"))
            })
            .collect();
        // Explicit reset+fixture-load sequence used by the autorun
        // shim. Mirrors Crystal's `test_setup.cr` (which emits a static
        // `RoundhouseTest.fixture_loaders = [-> { ArticlesFixtures.
        // _fixtures_load!; nil }, …]`). The dynamic `Object.constants`
        // walk that `FixtureLoader.load_all!` uses under CRuby is not
        // reachable under spinel-AOT; materializing the list at emit
        // time is the AOT-friendly analog.
        let mut reset_lines: Vec<String> = truncate_lines.clone();
        for lc in &fixture_lcs {
            reset_lines.push(format!("{}._fixtures_load!", lc.name.0.as_str()));
        }
        for lowered in &test_lowered {
            let lc = &lowered.test_class;
            let class_name = lc.name.0.as_str();
            let stem = test_file_stem(class_name);
            let dir = if class_name.ends_with("ControllerTest") {
                "controllers"
            } else {
                "models"
            };
            let out_path = PathBuf::from(format!("test/{dir}/{stem}_test.rb"));
            // Map both `ActiveSupport::TestCase` (Rails app tests) and
            // `Minitest::Test` (framework's own tests) to roundhouse-
            // owned `TestBase` (defined in test_helper.rb). Insulates
            // emitted tests from Minitest's class hierarchy —
            // assert_*/refute_* are already inlined as `raise` by the
            // inline_assertions lowerer, and the per-test shim
            // replaces Minitest's autorun. The zero-arg `X.new` the
            // shim emits requires this rewrite: `Minitest::Test#
            // initialize(name)` requires a method-name argument, so
            // inheriting from it directly breaks the shim under
            // CRuby. Ruby-target-specific rewrite, lives here rather
            // than in the lowerer.
            let mut lc_for_emit = lc.clone();
            if matches!(
                &lc.parent,
                Some(p) if p.0.as_str() == "ActiveSupport::TestCase"
                    || p.0.as_str() == "Minitest::Test"
            ) {
                lc_for_emit.parent = Some(crate::ident::ClassId(
                    crate::ident::Symbol::from("TestBase"),
                ));
            }
            let mut emitted = library::emit_library_class_decl_with_synthesized(
                &lc_for_emit,
                app,
                out_path,
                &fixture_siblings,
            );
            // Class-body constant assignments (`TABLE = [...]`) hoist
            // to file scope above the test class so bare-name refs
            // inside test methods resolve. CRuby's lexical constant
            // lookup finds top-level constants from anywhere; the
            // emitted form preserves the test's original semantics
            // without nesting back inside the class.
            if !lowered.constants.is_empty() {
                let mut consts_block = String::new();
                for (name, value) in &lowered.constants {
                    let value_s = super::ruby::expr::emit_expr(value);
                    writeln!(consts_block, "{} = {}", name.as_str(), value_s).unwrap();
                }
                consts_block.push('\n');
                emitted.content = splice_after_requires(&emitted.content, &consts_block);
            }
            // Inner classes (declared inside the test class body in
            // Ruby — `class Validatable; include …; end` inside
            // ValidationsTest) hoist to file scope above the test
            // class body so test methods that reference them by bare
            // name resolve. CRuby would also accept the original
            // nested form, but the IR has already flattened them via
            // `lower_test_modules_with_inner`; emitting at file scope
            // mirrors the TS pattern and keeps both targets aligned.
            if !lowered.inner_classes.is_empty() {
                let mut companion_block = String::new();
                for inner in &lowered.inner_classes {
                    let inner_rb_path = PathBuf::from("test")
                        .join(format!("{}_inner.rb", test_file_stem(inner.name.0.as_str())));
                    let inner_emitted = library::emit_library_class_decl_with_synthesized(
                        inner,
                        app,
                        inner_rb_path.clone(),
                        &fixture_siblings,
                    );
                    companion_block.push_str(&strip_require_headers(&inner_emitted.content));
                    companion_block.push('\n');
                    // RBS sidecar for the inner class. Its `.rb` body is
                    // spliced into the test file above (no standalone
                    // `.rb`), but RBS matches classes by name globally,
                    // so a standalone `sig/test/<stem>_inner.rbs` applies
                    // to the spliced `Article`. Without it spinel infers
                    // the stand-in's `[]` itself and mis-compiles the
                    // heterogeneous `Integer | String` return as one C
                    // type (matz/spinel#1255).
                    files.push(library::emit_rbs_sidecar(inner, &inner_rb_path));
                }
                // Splice companions ahead of the main class body but
                // after the file-level `require_relative` headers
                // produced by `emit_library_class_decl_with_synthesized`.
                emitted.content = splice_after_requires(&emitted.content, &companion_block);
            }
            // Test files need the bootstrap (minitest/autorun + runtime
            // requires + adapter setup) before any model require resolves;
            // prepend the require before the body-derived require headers.
            //
            // Also prepend explicit `require_relative` for every fixture
            // file. Test_helper's `FixtureLoader.load_all!` walks
            // `Object.constants` for `*Fixtures` modules; under spinel
            // AOT (no dynamic `Dir[…]` + `require`), each fixture must be
            // statically required from somewhere in the require chain
            // that spinel follows from the test file root. Injecting at
            // every test file guarantees coverage regardless of which
            // fixtures the body itself names.
            let mut preamble = String::from("require_relative \"../test_helper\"\n");
            for (_, anchor) in &fixture_siblings {
                // Test files live at `test/{models,controllers}/…`,
                // fixture anchors at `test/fixtures/<stem>`, so the
                // relative path from a test file's dir is always
                // `../fixtures/<stem>`.
                let stem = anchor
                    .strip_prefix("test/fixtures/")
                    .unwrap_or(anchor.as_str());
                writeln!(preamble, "require_relative \"../fixtures/{stem}\"").unwrap();
            }
            emitted.content = format!("{preamble}{}", emitted.content);
            emitted.content.push_str(&render_autorun_shim(&lc_for_emit, &reset_lines));
            let rbs_path = emitted.path.clone();
            files.push(emitted);
            // RBS sidecar for the test class — describes the test
            // methods' (untyped, () -> void) signatures plus any
            // inferred helpers. No preamble / autorun shim — RBS is
            // pure type info, the runtime bootstrap doesn't apply.
            files.push(library::emit_rbs_sidecar(&lc_for_emit, &rbs_path));
        }
    }

    // `app/models.rb` aggregator — loads every emitted app/models/*.rb.
    // Model files carry requires only for their LOAD-time deps
    // (superclass, includes, class-body constant refs); method-body
    // references between them resolve because main.rb / test_helper.rb
    // require this file before any dispatch (see the edge
    // classification in `library::emit_library_class_decl_with_
    // synthesized`). Emitted here so every consumer of `emit_spinel`
    // (the toolchain scratch trees included) satisfies those
    // unconditional requires; the project pipeline regenerates it
    // after `emit_library` adds the ingested support classes
    // (`apply_models_aggregator`).
    let mut model_anchors: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let p = f.path.to_str()?;
            if p.starts_with("app/models/") && p.ends_with(".rb") {
                Some(p.strip_suffix(".rb").unwrap().strip_prefix("app/").unwrap().to_string())
            } else {
                None
            }
        })
        .collect();
    model_anchors.sort();
    let mut aggregator = String::from(
        "# Loads every model/support class under app/models/. Generated\n\
         # from the emitted tree (see emit_spinel / apply_models_aggregator).\n\
         # Each file requires its own LOAD-time deps (superclass, includes,\n\
         # class-body constants), so the order here is only for legibility;\n\
         # method-body references between these files rely on this\n\
         # aggregator having run by dispatch time.\n",
    );
    for anchor in &model_anchors {
        writeln!(aggregator, "require_relative {anchor:?}").unwrap();
    }
    files.push(EmittedFile {
        path: PathBuf::from("app/models.rb"),
        content: aggregator,
    });

    files
}

/// Render RBS for a `LibraryFunction` group without emitting the .rb.
/// Used by `emit_spinel` for files whose .rb emit has custom
/// post-processing (e.g. `config/routes.rb`'s require-header
/// prepend) — the .rb flows through the bespoke path while the
/// `.rbs` is derived once from the same lowered functions.
fn rbs_only_from_funcs(
    funcs: &[crate::dialect::LibraryFunction],
    rb_path: PathBuf,
) -> Vec<EmittedFile> {
    if funcs.is_empty() {
        return Vec::new();
    }
    vec![library::emit_rbs_sidecar_from_funcs(funcs, &rb_path)]
}

/// Explicit per-test driver appended to each emitted test file.
/// Materializes the `reset → fixture-load → setup → test_X →
/// teardown` lifecycle for every `test_*` instance method in `lc`.
/// Replaces Minitest's at_exit autorun (not reachable under spinel-
/// AOT; would double-run with the shim under CRuby). See
/// spinel-AOT.
///
/// `reset_lines` carries pre-rendered `<Model>._adapter_truncate` +
/// `<X>Fixtures._fixtures_load!` lines (materialized once per emit;
/// see the caller in emit_spinel_with). Inlined rather than calling
/// `SchemaSetup.reset!` because the latter delegates to
/// `FixtureLoader.load_all!`, which walks `Object.constants`
/// dynamically — not reachable under spinel-AOT.
///
/// EVERY test runs, and a failing one does not take the rest of the
/// file with it. The straight-line form this replaced reported only a
/// file's FIRST failing test, which made the campfire suite 52
/// booleans over 240 tests — a file one wall from green and a file ten
/// walls from green were the same row, so a first-error histogram
/// ranked work by what fails FIRST rather than by what BLOCKS (two
/// consecutive mis-ranked probes; see the coverage-ladder notes).
///
/// Both contracts of the old shape survive:
///   * an all-green run prints EXACTLY `<Class>: <N> tests passed`,
///     which is what `project.rs`'s `.expected` snapshots pin;
///   * any failure still exits nonzero (the trailing `raise`), which
///     `make spinel-test` consumes as a fail signal.
///
/// `setup` runs inside the guard (minitest reports a setup error
/// against the test); `teardown` runs outside it so it fires whether
/// or not the body raised, and a raise from teardown itself still
/// aborts the file rather than being swallowed.
fn render_autorun_shim(lc: &LibraryClass, reset_lines: &[String]) -> String {
    let class_name = lc.name.0.as_str();
    let test_methods: Vec<&str> = lc
        .methods
        .iter()
        .filter(|m| matches!(m.receiver, MethodReceiver::Instance))
        .map(|m| m.name.as_str())
        .filter(|n| n.starts_with("test_"))
        .collect();

    let mut s = String::from(
        "\n# Spinel AOT autorun shim — see emit/ruby.rs::render_autorun_shim.\n",
    );
    // An Integer counter and per-failure `puts`, deliberately: no
    // array accumulator and no block, so the shim leans on nothing
    // beyond `begin/rescue`, `+`, `to_s` and string concatenation —
    // the surface spinel-AOT already compiles in lowered test bodies
    // (`assert_raises`) and `runtime/spinel`.
    writeln!(s, "__failed = 0").unwrap();
    for tm in &test_methods {
        // Zero-arg `.new` — mirrors Crystal's `@type.new.test_X` shape.
        // Spinel doesn't propagate inherited `Minitest::Test#
        // initialize(name)` to subclasses, and the @name slot isn't
        // used by any assertion the lowered tests reach, so dropping
        // it is safe.
        writeln!(s, "__t = {class_name}.new").unwrap();
        writeln!(s, "__ok = true").unwrap();
        writeln!(s, "begin").unwrap();
        // Truncate + fixture-load INSIDE the guard: they are this
        // shim's spelling of minitest's fixture setup, and a fixture
        // that fails to load is a per-test error in Rails. Outside
        // the guard it would abort the file and hide every test
        // behind it — the exact failure mode this shim exists to end.
        for line in reset_lines {
            writeln!(s, "  {line}").unwrap();
        }
        writeln!(s, "  __t.setup").unwrap();
        writeln!(s, "  __t.{tm}").unwrap();
        // `rescue Exception`, not a bare rescue: a bare one catches
        // StandardError, and an assertion library may deliberately put
        // its failure OUTSIDE that. `Mocha::ExpectationError <
        // Exception` exactly so a test's own `rescue => e` cannot
        // swallow an unmet expectation — and this shim's rescue was
        // swallowing nothing, it was MISSING it, so the error walked
        // out of the file and took the remaining tests with it.
        // Minitest's own runner rescues Exception here for the same
        // reason.
        writeln!(s, "rescue Exception => __e").unwrap();
        writeln!(s, "  __ok = false").unwrap();
        writeln!(s, "  __failed = __failed + 1").unwrap();
        writeln!(s, "  puts {:?} + __e.message", format!("FAIL {class_name}#{tm}: ")).unwrap();
        writeln!(s, "end").unwrap();
        // Teardown gets a guard of its OWN, for the reason the comment
        // above gives for the reset lines: unguarded, a raise here
        // aborts the FILE and hides every test behind it. It did —
        // `mocha_verify` raises on an unmet `expects`, which is a
        // failure of THIS test, and one of them took
        // `membership_test` from 8 of 9 to 0 of 9.
        //
        // Its own begin/end rather than moving it above the rescue:
        // teardown has to run even when the test raised, or a stub
        // installed on a real object outlives the test that made it.
        // `__ok` keeps a test that already failed from being counted
        // twice — `__failed` is the tally's numerator and must not
        // exceed the test count.
        writeln!(s, "begin").unwrap();
        writeln!(s, "  __t.teardown").unwrap();
        writeln!(s, "rescue Exception => __e").unwrap();
        writeln!(s, "  if __ok").unwrap();
        writeln!(s, "    __failed = __failed + 1").unwrap();
        writeln!(
            s,
            "    puts {:?} + __e.message",
            format!("FAIL {class_name}#{tm}: ")
        )
        .unwrap();
        writeln!(s, "  end").unwrap();
        writeln!(s, "end").unwrap();
    }
    let n = test_methods.len();
    writeln!(s, "if __failed > 0").unwrap();
    writeln!(
        s,
        "  raise {:?} + __failed.to_s + {:?}",
        format!("{class_name}: "),
        format!(" of {n} tests failed")
    )
    .unwrap();
    writeln!(s, "end").unwrap();
    writeln!(s, "puts {:?}", format!("{class_name}: {n} tests passed")).unwrap();
    s
}

/// Drop the leading `require_relative "..."` lines from an emitted
/// class file's content, leaving just the class body. Used when
/// splicing a companion class into a host file — the host already
/// emits its own require headers, and the companion's headers would
/// either duplicate or land in the wrong order.
fn strip_require_headers(content: &str) -> String {
    let lines = content.lines();
    let mut body_start = 0usize;
    let mut idx = 0usize;
    for line in content.lines() {
        idx += line.len() + 1;
        let trimmed = line.trim();
        if trimmed.starts_with("require_relative ") || trimmed.is_empty() {
            body_start = idx;
            continue;
        }
        break;
    }
    let _ = lines; // silence lint
    content[body_start..].to_string()
}

/// Insert `block` into `content` right after the trailing
/// `require_relative` headers (and the blank line that separates
/// them from the body). When `content` has no requires, the block
/// is prepended.
fn splice_after_requires(content: &str, block: &str) -> String {
    let mut split_at = 0usize;
    let mut idx = 0usize;
    let mut last_require_end = 0usize;
    for line in content.lines() {
        let line_end = idx + line.len() + 1;
        let trimmed = line.trim();
        if trimmed.starts_with("require_relative ") {
            last_require_end = line_end;
        } else if trimmed.is_empty() && last_require_end > 0 && last_require_end == idx {
            // Blank line directly after the last require — splice
            // after this blank so spacing reads naturally.
            split_at = line_end;
            break;
        } else if !trimmed.is_empty() {
            split_at = last_require_end;
            break;
        }
        idx = line_end;
    }
    if split_at == 0 {
        split_at = last_require_end;
    }
    if split_at == 0 {
        return format!("{block}\n{content}");
    }
    let (head, tail) = content.split_at(split_at);
    format!("{head}{block}\n{tail}")
}

/// `ArticlesFixtures` → `articles` (strip Fixtures suffix, snake_case).
/// Mirrors `typescript.rs:fixture_file_stem` so the emitted file path
/// reads naturally without redundant suffixes.
fn fixture_file_stem(class_name: &str) -> String {
    let stem = class_name.strip_suffix("Fixtures").unwrap_or(class_name);
    crate::naming::snake_case(stem)
}

/// Render `test/test_helper.rb`, substituting the source file's
/// `Object.constants.sort.each` + `Object.const_get` scan in
/// `FixtureLoader.load_all!` with explicit per-fixture calls. The
/// scan was a CRuby-only convenience (the spinel subset rejects
/// `Object.const_get` and `Object.constants`); we already know the
/// fixture set at emit time, so emit the explicit list and let the
/// source file keep the const_get fallback for hand-written non-emit
/// uses.
///
/// Class names are sorted alphabetically — the same ordering the
/// source-side scan used (which approximates parent-before-child for
/// the `Articles → Comments` belongs_to shape; topological ordering
/// is the principled fix once a fixture set exposes a non-alphabetic
/// dependency).
/// Render `test/test_helper.rb` for an emitted app.
///
/// The source file at `runtime/spinel/test/test_helper.rb` has to run
/// verbatim under the source-side `framework_ruby_tests_pass` gate,
/// where there is no app and no `main.rb` — so it carries a
/// blog-shaped stand-in for each of the three things that are really
/// per-app, and each is substituted here. Everything blog-specific in
/// this harness is one of these three; if a fourth appears, it belongs
/// here rather than in the source file.
///
///   1. the runtime require chain → `require_relative "../main"`
///   2. `SchemaSetup.reset!`'s truncate list → the app's own models
///   3. `FixtureLoader.load_all!`'s constant scan → explicit calls
fn render_test_helper(fixture_lcs: &[LibraryClass], truncate_lines: &[String]) -> String {
    // (1) The require chain. The source list is a hand-maintained
    // SUBSET that stopped tracking the runtime as it grew: by
    // 2026-08-16 `boot.rb` required 23 files this did not, so a
    // harness-loaded app died at `uninitialized constant ActionText`
    // — a require gap that reads exactly like a modeling gap.
    //
    // Defers to `boot.rb`, which is the app's whole chain in its
    // curated order and NOTHING that starts anything. Deliberately not
    // `main.rb`: the spinel-AOT main.rb boots the server
    // unconditionally (the `__FILE__ == $PROGRAM_NAME` guard cannot
    // work under AOT — see its own comment), so a test that required it
    // opened the dev database and died at `sqlite3_open(…) failed (14)`
    // before running an assertion. boot.rb is the half both can share.
    const REQUIRE_BLOCK: &str = "require_relative \"../runtime/base64\"";
    const REQUIRE_BLOCK_END: &str = "require_relative \"../app/models\"";

    // (2) Per-test isolation. `_adapter_truncate` exists only on models
    // with a backing schema table; the caller applies that gate.
    const TRUNCATE_BLOCK: &str = "    Article._adapter_truncate if defined?(Article)
    Comment._adapter_truncate if defined?(Comment)";

    // (3) Fixture loading — spinel's AOT model has no runtime constant
    // table, so the scan is materialized at emit time.
    const SCAN_BLOCK: &str = "    Object.constants.sort.each do |c|
      next unless c.to_s.end_with?(\"Fixtures\")
      mod = Object.const_get(c)
      next unless mod.is_a?(Module)
      next unless mod.respond_to?(:_fixtures_load!)
      mod._fixtures_load!
    end";

    debug_assert!(
        SPINEL_TEST_HELPER.contains(SCAN_BLOCK),
        "runtime/spinel/test/test_helper.rb FixtureLoader.load_all! body \
         changed; update SCAN_BLOCK in render_test_helper"
    );
    debug_assert!(
        SPINEL_TEST_HELPER.contains(TRUNCATE_BLOCK),
        "runtime/spinel/test/test_helper.rb SchemaSetup.reset! truncate \
         list changed; update TRUNCATE_BLOCK in render_test_helper"
    );

    let mut out = SPINEL_TEST_HELPER.to_string();

    // (1) — replace the whole span, comments included, so no stale
    // rationale survives next to the line that replaced it.
    let (Some(start), Some(end)) = (out.find(REQUIRE_BLOCK), out.find(REQUIRE_BLOCK_END)) else {
        debug_assert!(
            false,
            "runtime/spinel/test/test_helper.rb require chain changed; \
             update REQUIRE_BLOCK/_END in render_test_helper"
        );
        return out;
    };
    out.replace_range(
        start..end + REQUIRE_BLOCK_END.len(),
        "# The app's whole boot chain, in its curated order: the runtime,\n\
         # config/schema, config/routes, app/models, app/views. boot.rb\n\
         # starts nothing — main.rb requires it too, then adds the server.\n\
         # One owner for the order; a harness-local copy of the list\n\
         # drifts, and did (23 files short, read as a modeling gap).\n\
         require_relative \"../boot\"",
    );

    // (2)
    let truncate = if truncate_lines.is_empty() {
        String::from("    # no table-backed models")
    } else {
        truncate_lines
            .iter()
            .map(|line| format!("    {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    out = out.replace(TRUNCATE_BLOCK, &truncate);

    // (3)
    // `fixture_lcs` ORDER, not sorted. The lowerer topologically sorts
    // the fixtures parent-before-child (`fixture_to_library::
    // load_order`); re-sorting here alphabetically threw that away and
    // is how campfire loaded `users` last, after the five fixtures that
    // reference one. Two owners for one order, and the second had no
    // idea the first had computed anything.
    let names: Vec<&str> = fixture_lcs.iter().map(|lc| lc.name.0.as_str()).collect();
    let explicit = if names.is_empty() {
        // Empty FixtureLoader.load_all! body: no fixtures to load.
        // Keep the indentation consistent so the substitution result
        // is still valid Ruby (a `def`/`end` with no statements).
        String::from("    # no fixtures")
    } else {
        names
            .iter()
            .map(|name| format!("    {name}._fixtures_load!"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    out.replace(SCAN_BLOCK, &explicit)
}

/// `ArticleTest` → `article`, `ArticlesControllerTest` →
/// `articles_controller` (strip Test suffix, snake_case). Used for the
/// `test/<dir>/<stem>_test.rb` output path.
/// File stem for a test program, with the namespace FLATTENED into the
/// name: `Rooms::ClosedsControllerTest` → `rooms_closeds_controller`.
///
/// `snake_case` alone passes `::` straight through — the same trap
/// 147d5848 fixed for model/controller output paths and require anchors,
/// which the test path never got. It produced
/// `test/rooms::closeds_controller_test.rb`, and a `::` in a filename is
/// not merely ugly: the emitted Makefile globs `*.rbs` into `RBS_SRC`
/// and lists the programs in `SPINEL_TESTS`, and a `::` in a Make
/// prerequisite parses as a second target separator —
/// `Makefile:149: *** multiple target patterns.  Stop.` campfire emits
/// 33 such tests, so `make build` failed before spinel saw a line of it.
///
/// Flattened with `_` rather than nested with `underscore()`'s `/`,
/// because the spin relocation moves an in-lane test to `test/<basename>`
/// (`apply_spinel_test_layout`): a nested path would collapse to its last
/// segment there, so `Rooms::ClosedsControllerTest` and
/// `Accounts::ClosedsControllerTest` would both land on
/// `closeds_controller_test.rb` and one would overwrite the other.
fn test_file_stem(class_name: &str) -> String {
    let stem = class_name.strip_suffix("Test").unwrap_or(class_name);
    crate::naming::underscore(stem).replace('/', "_")
}


#[cfg(test)]
mod method_sig_tests {
    use super::*;
    use crate::dialect::{AccessorKind, Param};
    use crate::effect::EffectSet;
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ident::Symbol;
    use crate::span::Span;

    // `def get_from_cache(opts = {}, &block)` — an optional positional with
    // a default plus a captured block param must both round-trip into the
    // emitted signature (regression: both were dropped → `def f` + arity
    // crash / undefined `block`). See the lobsters runtime-wiring probe.
    #[test]
    fn emit_method_renders_optional_default_and_block_param() {
        let m = MethodDef {
            name: Symbol::from("get_from_cache"),
            receiver: MethodReceiver::Instance,
            params: vec![Param::with_default(
                Symbol::from("opts"),
                Expr::new(Span::synthetic(), ExprNode::Hash { entries: vec![], kwargs: false }),
            )],
            body: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: Some(Param::positional(Symbol::from("block"))),
        };
        let out = emit_method(&m);
        assert!(
            out.contains("def get_from_cache(opts = {}, &block)"),
            "got:\n{out}"
        );
    }
}
