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
//! - Controllers, dispatch, and the route table ride the universal-IR
//!   overlay (`overlay.rs`, emitted under `app/v2/`) — the live
//!   dispatch path since the CtrlWalker retirement.
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
//! external surface that site generation (`roundhouse --site`) uses).

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::dialect::MethodDef;
use crate::ty::Ty;

mod expr;
mod importmap;
mod library;
mod main;
pub mod overlay;
mod pyproject;
mod route;
mod schema_sql;
mod shared;
mod ty;

// External API: kept for anything that keys off `python_ty` directly.
pub use ty::python_ty;

// Framework-runtime transpile surface, consumed by
// `runtime_loader::python_units` (the `PYTHON_TARGET` hooks). Partially
// live: `emit()` ships the units named in `LIVE_FRAMEWORK` via
// `python_units_subset`; the hand-written `runtime/python/*.py` files
// are strangled one at a time as entries graduate into that list.
pub use library::{emit_expr_for_runtime, emit_library_class, emit_module};

/// Emit a typed `MethodDef` as a standalone Python function
/// (trailing newline included). Requires `signature` to be populated
/// — `parse_methods_with_rbs` does this.
pub fn emit_method(m: &MethodDef) -> String {
    let sig = m
        .signature
        .as_ref()
        .expect("emit_method requires a signature");
    let Ty::Fn { params: sig_params, ret, .. } = sig else {
        panic!("signature is not Ty::Fn");
    };
    assert_eq!(
        sig_params.len(),
        m.params.len(),
        "method `{}`: signature/param arity mismatch",
        m.name
    );

    let param_list: Vec<String> = m
        .params
        .iter()
        .zip(sig_params.iter())
        .map(|(name, p)| format!("{}: {}", name, python_ty(&p.ty)))
        .collect();

    let ret_s = python_ty(ret);
    let body = expr::emit_body(&m.body, ret);

    let mut out = String::new();
    writeln!(out, "def {}({}) -> {}:", m.name, param_list.join(", "), ret_s).unwrap();
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "    {line}").unwrap();
        }
    }
    out
}

const DB_SOURCE: &str = include_str!("../../runtime/python/db.py");
const PARAMS_SOURCE: &str = include_str!("../../runtime/python/params.py");
/// Native-`datetime` seam for temporal columns: `Roundhouse.RhDateTime.
/// parse` (stored ISO-8601 text -> datetime, the `parse_db_time` intrinsic
/// target) plus a module-load patch that gives `json_builder.encode_datetime`
/// a native-`datetime` dispatch (Rails' canonical `...Z` millisecond JSON).
/// Ships as `app/rh_datetime.py`; imported by models.py when a temporal
/// column's reader `@property` references `Roundhouse.RhDateTime`.
const RH_DATETIME_SOURCE: &str = include_str!("../../runtime/python/rh_datetime.py");
/// Python HTTP runtime — the `ActionResponse` value type
/// `app/v2/dispatch.handle` returns and the server/TestClient glue
/// consumes. Copied verbatim into generated projects as
/// `app/http.py` when any controller emits.
const HTTP_SOURCE: &str = include_str!("../../runtime/python/http.py");
/// Pass-2 test-support runtime. `TestClient` + `TestResponse` with
/// Rails-shaped assertions. Ships as `app/test_support.py`.
const TEST_SUPPORT_SOURCE: &str =
    include_str!("../../runtime/python/test_support.py");
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

/// Framework leaf files that have completed the strangler switchover:
/// shipped as transpiled `runtime/ruby/*` output instead of hand-written
/// `runtime/python/*.py`. Switchover-ready means more than `py_compile`
/// clean — the file must be free of `report_unsupported` degrades (those
/// trip the transpile fail-policy) AND import cleanly in the project.
///
/// All eight files below transpile degrade-free after the Super / Yield /
/// Return / runtime-ism work. `view_helpers` ships transpiled too, but at
/// the overlay's own path (`app/v2/view_helpers.py`, see
/// `overlay::emit_overlay_files`) — the hand-written twin was deleted
/// with the per-artifact retirement.
const LIVE_FRAMEWORK: &[&str] = &[
    "app/inflector.py",
    "app/json_builder.py",
    "app/errors.py",
    "app/active_record_base.py",
    "app/flash.py",
    "app/session.py",
    "app/action_controller_base.py",
    "app/router.py",
];

/// Emit the switched-over framework leaf files (see `LIVE_FRAMEWORK`).
fn live_framework_units() -> Vec<EmittedFile> {
    // Filter at the source: emitting a dormant degrade-heavy entry would
    // fire its diagnostics into the shared sink and fail the transpile
    // policy even if its output were dropped. The identity transform is
    // the tree-shake seam (unused — Python has no tree-shake yet).
    crate::runtime_loader::python_units_subset(LIVE_FRAMEWORK, |_, classes| classes)
        .map(|units| {
            units
                .into_iter()
                .map(|u| EmittedFile { path: u.out_path, content: u.content })
                .collect()
        })
        .unwrap_or_default()
}

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    if !app.models.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("app/db.py"),
            content: DB_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/params.py"),
            content: PARAMS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/rh_datetime.py"),
            content: RH_DATETIME_SOURCE.to_string(),
        });
        files.push(schema_sql::emit_schema_sql(app));
        files.push(EmittedFile {
            path: PathBuf::from("app/__init__.py"),
            content: String::new(),
        });
        files.extend(live_framework_units());
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
        // The universal-IR overlay (app/v2/) — the LIVE dispatch path:
        // server.py and TestClient route every request through
        // app/v2/dispatch.handle. The per-artifact controllers above
        // still ship but are no longer routed to; they retire in
        // Phase E (docs/python-overlay-plan.md).
        match overlay::emit_overlay_files(app) {
            Ok(v2) => files.extend(v2),
            Err(e) => panic!("python overlay emit failed: {e}"),
        }
    }
    if !app.test_modules.is_empty() {
        // tests/ needs __init__.py so unittest discovery works; the
        // test files themselves ride the overlay emission.
        files.push(EmittedFile {
            path: PathBuf::from("tests/__init__.py"),
            content: String::new(),
        });
    }
    files
}
