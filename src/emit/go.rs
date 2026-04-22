//! Go emitter.
//!
//! Second typed target. Differences from Rust that surface design choices:
//!
//! - Go has no Option / Result — nullable types are pointers (`*T`).
//! - Go has no class methods — `Post.All()` is pseudo-Go; a real mapping
//!   would use a repository pattern or package-level functions.
//! - Go requires explicit `return` for non-void functions and omits the
//!   return type entirely for void functions.
//! - Go convention: `ID`, `URL`, `HTTP` for initialism fields; PascalCase
//!   for exported identifiers, camelCase for unexported.
//! - Go uses tabs for indentation; gofmt would realign struct fields.
//!   We emit single-tab indent without alignment.
//!
//! Output is pseudo-Go — won't compile as-is. The goal is to prove that
//! types flow through to a second typed target without the Rust emitter
//! accidentally hiding ambiguities.
//!
//! Organized into one submodule per output kind. Cross-cutting helpers
//! live in `shared`; the generic body/expression walker lives in `expr`
//! and is reused by the model-method, view, and test emitters; type
//! rendering lives in `ty` (with `go_ty` re-exported here for any
//! external surface that may key off it).

use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::ident::Symbol;

mod controller;
mod controller_test;
mod expr;
mod fixture;
mod gomod;
mod importmap;
mod main;
mod model;
mod route;
mod schema_sql;
mod shared;
mod spec;
mod ty;
mod view;

// External API — `bin/build-site` consumes `emit`. `go_ty` is
// re-exported in case downstream callers key off it the way they
// do for `crystal_ty`.
pub use ty::go_ty;

const RUNTIME_SOURCE: &str = include_str!("../../runtime/go/runtime.go");
const DB_SOURCE: &str = include_str!("../../runtime/go/db.go");
/// Go HTTP runtime — Phase 4d pass-2 shape. Copied verbatim into
/// generated projects as `app/http.go` whenever any controller emits.
/// Provides ActionResponse, ActionContext, Router, plus Phase 4c
/// compile-only stubs for any leftover legacy references.
const HTTP_SOURCE: &str = include_str!("../../runtime/go/http.go");
/// Go test-support runtime — TestClient + TestResponse. Copied
/// verbatim as `app/test_support.go` whenever controllers emit.
const TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/go/test_support.go");
/// Go view-helpers runtime — link_to, button_to, FormBuilder,
/// turbo_stream_from, dom_id, pluralize, plus set_yield/slot
/// storage for layout dispatch. Copied verbatim as
/// `app/view_helpers.go`.
const VIEW_HELPERS_SOURCE: &str = include_str!("../../runtime/go/view_helpers.go");
/// Go net/http server runtime — `Start(StartOptions)` dispatches
/// through `Router.Match`, wraps HTML responses in the emitted
/// layout, handles `_method` override for Rails forms. Copied
/// verbatim as `app/server.go`.
const SERVER_SOURCE: &str = include_str!("../../runtime/go/server.go");
/// Go cable runtime — Action Cable WebSocket + Turbo Streams
/// broadcaster. Mirrors runtime/rust/cable.rs +
/// runtime/python/cable.py: actioncable-v1-json subprotocol,
/// per-channel subscriber map, partial-renderer registry. Copied
/// as `app/cable.go`.
const CABLE_SOURCE: &str = include_str!("../../runtime/go/cable.go");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(gomod::emit_go_mod());
    files.push(gomod::emit_go_sum());
    if !app.models.is_empty() {
        files.push(model::emit_models(app));
        files.push(EmittedFile {
            path: PathBuf::from("app/runtime.go"),
            content: RUNTIME_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/db.go"),
            content: DB_SOURCE.to_string(),
        });
        files.push(schema_sql::emit_schema_sql_go(app));
    }
    if !app.controllers.is_empty() {
        // HTTP runtime + TestClient — copied verbatim, same posture as
        // runtime.go / db.go. Provides ActionResponse, ActionContext,
        // Router, and (still) the Phase 4c stubs.
        files.push(EmittedFile {
            path: PathBuf::from("app/http.go"),
            content: HTTP_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/test_support.go"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/view_helpers.go"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/server.go"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/cable.go"),
            content: CABLE_SOURCE.to_string(),
        });
        let known_models: Vec<Symbol> =
            app.models.iter().map(|m| m.name.0.clone()).collect();
        for c in &app.controllers {
            files.push(controller::emit_controller_pass2(c, &known_models, app));
        }
        files.push(route::emit_go_route_helpers(app));
        files.push(route::emit_go_routes(app));
        files.push(importmap::emit_go_importmap(app));
        files.push(main::emit_go_main(app));
        files.push(view::emit_go_views(app, &known_models));
    }
    if !app.fixtures.is_empty() {
        files.push(fixture::emit_go_fixtures(app));
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(spec::emit_go_tests(tm, app));
        }
    }
    files
}
