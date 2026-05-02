//! Ruby emitter: App → a set of Ruby source files.
//!
//! The reverse direction of Prism ingest. Together they form the round-trip
//! forcing function: Ruby source → IR → Ruby source should preserve semantics.
//!
//! Organized into one submodule per output kind. Cross-cutting helpers live
//! in `shared`; expression emission lives in `expr` and is reused by all the
//! per-form modules.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::dialect::{MethodDef, MethodReceiver};

mod controller;
mod expr;
mod fixture;
mod importmap;
mod library;
mod model;
mod route;
mod schema;
mod seeds;
mod shared;
mod test;
mod view;

// External API: the historical surface kept for `tests/` and `bin/`.
pub use expr::emit_expr;
pub use view::reconstruct_erb;

/// Emit a single `MethodDef` as Ruby source (trailing newline included).
/// The signature and effects are not emitted — they belong to the RBS
/// sidecar, not to Ruby itself. Used by the runtime-extraction pipeline
/// to round-trip a typed standalone function back to Ruby source.
pub fn emit_method(m: &MethodDef) -> String {
    let prefix = match m.receiver {
        MethodReceiver::Instance => "",
        MethodReceiver::Class => "self.",
    };
    let params = if m.params.is_empty() {
        String::new()
    } else {
        let ps: Vec<String> = m
            .params
            .iter()
            .map(|p| match &p.default {
                Some(default) => format!("{} = {}", p.name.as_str(), expr::emit_expr(default)),
                None => p.name.as_str().to_string(),
            })
            .collect();
        format!("({})", ps.join(", "))
    };
    let mut out = String::new();
    writeln!(out, "def {prefix}{}{}", m.name, params).unwrap();
    let body_text = emit_expr(&m.body);
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
pub fn emit_lowered_models(app: &App) -> Vec<EmittedFile> {
    app.models
        .iter()
        .map(|m| {
            let lc = crate::lower::lower_model_to_library_class(m, &app.schema);
            let stem = crate::naming::snake_case(lc.name.0.as_str());
            let out_path = PathBuf::from(format!("app/models/{stem}.rb"));
            library::emit_library_class_decl(&lc, app, out_path)
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

/// Emit `config/routes.rb` in spinel-blog shape — a `Routes` module
/// `Routes` module exposing the dispatch data via class methods:
/// `Routes.table` returns the array of `{method:, pattern:,
/// controller:, action:}` hashes; `Routes.root` returns the
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
    for r in &flat {
        let class_name = r.controller.0.as_str();
        let stem = crate::naming::snake_case(class_name);
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
/// `app/controllers/<name>.rb` per controller.
///
/// What this pass DOESN'T cover (each is a follow-on lowerer): action-
/// body rewrites such as `params` → `@params`, polymorphic
/// `redirect_to @x` → `RouteHelpers.x_path(@x.id)`, and
/// `Article.includes(:foo).order(...)` → `.all` + in-memory sort.
pub fn emit_lowered_controllers(app: &App) -> Vec<EmittedFile> {
    controller::emit_lowered_controllers(app)
}

/// Lower each `app.views` entry through `view_to_library` and emit
/// the resulting `LibraryClass` as a Ruby source file under
/// `app/views/<dir>/<base>.rb`. Output is the universal post-lowering
/// shape: a `Views::<Plural>` module with one `def self.<action>(args)`
/// per view, body in `io = String.new ; io << ViewHelpers.x(...) ; io`
/// form. See `project_universal_post_lowering_ir.md`.
pub fn emit_lowered_views(app: &App) -> Vec<EmittedFile> {
    app.views
        .iter()
        .map(|v| {
            let lc = crate::lower::lower_view_to_library_class(v, app);
            let out_path = view_output_path(v.name.as_str());
            library::emit_library_class_decl(&lc, app, out_path)
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

/// Spinel-shape emit: lowered IR rendered as runnable Ruby. Composes
/// the five `emit_lowered_*` functions into a single project — schema,
/// routes, models, controllers, views — matching `fixtures/spinel-blog`'s
/// directory shape. The natural validation target of the lowering
/// pipeline (per `project_lowerers_first_validate_via_spinel.md`):
/// CRuby executes the output, and spinel-blog's hand-written tests
/// serve as the contract until spinel grows its own test runner.
pub fn emit_spinel(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(emit_lowered_schema(app));
    files.push(emit_lowered_routes(app));
    let importmap_funcs = crate::lower::lower_importmap_to_library_functions(app);
    if !importmap_funcs.is_empty() {
        files.push(library::emit_module_file(
            &importmap_funcs,
            app,
            PathBuf::from("config/importmap.rb"),
        ));
    }
    files.extend(emit_lowered_models(app));
    files.extend(emit_lowered_controllers(app));
    files.extend(emit_lowered_views(app));

    // RouteHelpers — `app/route_helpers.rb` with `def self.<x>_path(args)`
    // per named route. Generated from `app.routes`; supersedes the
    // hand-written `runtime/ruby/action_view/route_helpers.rb` (which
    // is being kept for backward compat until callers migrate).
    let route_helper_funcs = crate::lower::lower_routes_to_library_functions(app);
    if !route_helper_funcs.is_empty() {
        files.push(library::emit_module_file(
            &route_helper_funcs,
            app,
            PathBuf::from("app/route_helpers.rb"),
        ));
    }

    // Seeds — `db/seeds.rb` as a `Seeds.run` module method. Mirrors
    // the TS pipeline; was previously missing from spinel emit.
    let seeds_funcs = crate::lower::lower_seeds_to_library_functions(app);
    if !seeds_funcs.is_empty() {
        files.push(library::emit_module_file(
            &seeds_funcs,
            app,
            PathBuf::from("db/seeds.rb"),
        ));
    }

    files
}

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    if !app.schema.tables.is_empty() {
        files.push(schema::emit_schema(&app.schema));
    }
    for m in &app.models {
        files.push(model::emit_model(m));
    }
    for c in &app.controllers {
        files.push(controller::emit_controller(c));
    }
    files.push(route::emit_routes(&app.routes));
    for v in &app.views {
        files.push(view::emit_view(v));
    }
    for tm in &app.test_modules {
        files.push(test::emit_test_module(tm));
    }
    for f in &app.fixtures {
        files.push(fixture::emit_fixture(f));
    }
    if let Some(seeds) = &app.seeds {
        files.push(seeds::emit_seeds(seeds));
    }
    if let Some(im) = &app.importmap {
        files.push(importmap::emit_importmap(im));
    }
    // Preserve the discovered stylesheet list for round-trip by
    // emitting placeholder `.css` files. The content is empty on
    // purpose — the files act as a manifest that re-ingest
    // rediscovers, nothing more. A production Ruby emit would
    // copy real stylesheet content; we're aiming at IR fidelity
    // here, not asset pipeline reproduction.
    for name in &app.stylesheets {
        files.push(EmittedFile {
            path: PathBuf::from(format!("app/assets/stylesheets/{name}.css")),
            content: String::new(),
        });
    }
    files
}
