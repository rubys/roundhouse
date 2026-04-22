//! Python emitter.
//!
//! Third Phase 2 scaffold. Python and Ruby share a lot of surface
//! grammar (snake_case identifiers, `class`, dynamic typing), which
//! lets the emitter be thin. The interesting parts are the shape
//! choices:
//!
//! - Models as classes with type-hinted fields (`id: int`). No
//!   `@dataclass` yet — constructors and defaults are a Phase 3
//!   runtime concern.
//! - Controllers as classes with `async def` per action. Railcar's
//!   Python output uses module-level `async def` functions (aiohttp
//!   handler convention). The class shape is a deliberate choice for
//!   scaffold consistency with TS; Phase 3 can pivot to module-level
//!   if the Juntos-equivalent Python runtime prefers it.
//! - Routes as a `list[dict]` dispatch table.
//!
//! Python-specific idioms:
//! - `from __future__ import annotations` so forward references in
//!   type hints work without runtime import order concerns.
//! - `int | None` syntax for optional types (PEP 604, Python 3.10+).
//! - `list[T]` / `dict[K, V]` (PEP 585, Python 3.9+). No `typing.List`.
//! - Ruby symbols → string literals (same as TS).
//!
//! Organized into one submodule per output kind. Cross-cutting helpers
//! live in `shared`; the generic `Expr` walker lives in `expr` and is
//! reused by the model-method emitter and the controller fallback; type
//! rendering lives in `ty` (with `python_ty` re-exported here for the
//! external surface that `bin/build-site` uses).

use std::path::PathBuf;

use super::EmittedFile;
use crate::App;

mod controller;
mod controller_test;
mod expr;
mod fixture;
mod importmap;
mod main;
mod model;
mod pyproject;
mod route;
mod schema_sql;
mod shared;
mod spec;
mod ty;
mod view;

// External API: kept for anything that keys off `python_ty` directly.
pub use ty::python_ty;

const RUNTIME_SOURCE: &str = include_str!("../../runtime/python/runtime.py");
const DB_SOURCE: &str = include_str!("../../runtime/python/db.py");
/// Python HTTP runtime — Phase 4d pass-2 shape. `ActionResponse`,
/// `ActionContext`, and the Router match table live here; copied
/// verbatim into generated projects as `app/http.py` when any
/// controller emits. Mirrors the six sibling twins.
const HTTP_SOURCE: &str = include_str!("../../runtime/python/http.py");
/// Pass-2 test-support runtime. `TestClient` + `TestResponse` with
/// Rails-shaped assertions. Ships as `app/test_support.py`.
const TEST_SUPPORT_SOURCE: &str =
    include_str!("../../runtime/python/test_support.py");
/// View helpers — `link_to`, `button_to`, `FormBuilder`, etc.
/// Ships as `app/view_helpers.py` when views emit.
const VIEW_HELPERS_SOURCE: &str =
    include_str!("../../runtime/python/view_helpers.py");
/// aiohttp-based HTTP server + /cable route + method-override +
/// layout-wrap. Ships as `app/server.py` when controllers emit so
/// `uv run python3 -m app` (via the emitted `__main__.py` +
/// `pyproject.toml`) can serve both HTTP and WebSocket on one
/// event loop.
const SERVER_SOURCE: &str = include_str!("../../runtime/python/server.py");
/// Action Cable runtime — WebSocket handler + Turbo Streams
/// broadcaster. Always shipped alongside the server; models with
/// `broadcasts_to` call `crate::cable::broadcast_*_to` from their
/// save/destroy methods.
const CABLE_SOURCE: &str = include_str!("../../runtime/python/cable.py");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    if !app.models.is_empty() {
        files.push(model::emit_models(app));
        files.push(EmittedFile {
            path: PathBuf::from("app/runtime.py"),
            content: RUNTIME_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/db.py"),
            content: DB_SOURCE.to_string(),
        });
        files.push(schema_sql::emit_schema_sql_py(app));
        files.push(EmittedFile {
            path: PathBuf::from("app/__init__.py"),
            content: String::new(),
        });
    }
    if !app.controllers.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("app/http.py"),
            content: HTTP_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/test_support.py"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/view_helpers.py"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/server.py"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/cable.py"),
            content: CABLE_SOURCE.to_string(),
        });
        files.push(pyproject::emit_py_pyproject());
        files.push(route::emit_py_route_helpers(app));
        files.push(importmap::emit_py_importmap(app));
        files.push(main::emit_py_main(app));
        files.extend(controller::emit_controllers_pass2(app));
    }
    files.extend(view::emit_py_views(app));
    if !app.routes.entries.is_empty() {
        files.push(route::emit_routes(app));
    }
    if !app.fixtures.is_empty() {
        files.push(fixture::emit_py_fixtures(app));
        // tests/ needs __init__.py so unittest can discover
        files.push(EmittedFile {
            path: PathBuf::from("tests/__init__.py"),
            content: String::new(),
        });
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(spec::emit_py_test(tm, app));
        }
        if app.fixtures.is_empty() {
            files.push(EmittedFile {
                path: PathBuf::from("tests/__init__.py"),
                content: String::new(),
            });
        }
    }
    files
}
