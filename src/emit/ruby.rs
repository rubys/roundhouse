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
        let ps: Vec<&str> = m.params.iter().map(|p| p.as_str()).collect();
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
