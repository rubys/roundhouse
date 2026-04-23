//! Elixir emitter.
//!
//! Second Phase 2 scaffold. Elixir is the target that most aggressively
//! stress-tests IR target-neutrality because its paradigm is
//! fundamentally different from every other target on the list:
//!
//! - No classes — models are modules with a `defstruct` payload.
//! - No method dispatch — what Ruby calls `article.title` becomes
//!   module-function-on-record: `Article.title(article)` (or struct
//!   field access, `article.title`, for direct attributes).
//! - No mutation — ivar rebinds become variable rebinds (no `self.foo =`
//!   state threading at scaffold depth; real Elixir code returns the
//!   updated struct).
//! - No inheritance — the `parent` field on Model/Controller is noted
//!   but not emitted. Rails' `ApplicationRecord` / `ApplicationController`
//!   become `use Railcar.Record` / `use Railcar.Controller`-style
//!   conventions in a real runtime; the scaffold doesn't commit yet.
//! - Pattern matching as control flow — `if expr.save, do: …, else: …`
//!   is idiomatic as a `case` on `{:ok, _} / {:error, _}`. Scaffold
//!   emits `if/else` for now; Phase 3 runtime work converts.
//!
//! Non-goals:
//! - `@spec` type annotations (Elixir is dynamically typed; `Ty` info
//!   is useful for the Rust/Go/TS targets, not here).
//! - Phoenix / Plug integration.
//! - Live View / template emission.
//! - Controllers that return `{:cont, conn}` tuples (real Plug shape).

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::ident::Symbol;

mod controller;
mod expr;
mod fixture;
mod importmap;
mod main;
mod mix;
mod model;
mod route;
mod schema_sql;
mod shared;
mod spec;
mod view;

const RUNTIME_SOURCE: &str = include_str!("../../runtime/elixir/runtime.ex");
const DB_SOURCE: &str = include_str!("../../runtime/elixir/db.ex");
/// Elixir HTTP runtime — Phase 4d pass-2 shape. Copied verbatim
/// into generated projects as `lib/roundhouse/http.ex` when any
/// controller emits. Exposes ActionResponse/ActionContext structs
/// + Router.match table; the emitter's action templates return
/// ActionResponse directly (no class-based dispatch).
const HTTP_SOURCE: &str = include_str!("../../runtime/elixir/http.ex");
/// Pass-2 test-support runtime. TestClient + TestResponse with
/// Rails-shaped assertions. Ships as
/// `lib/roundhouse/test_support.ex`.
const TEST_SUPPORT_SOURCE: &str =
    include_str!("../../runtime/elixir/test_support.ex");
/// View helpers — link_to, button_to, FormBuilder, etc. Ships as
/// `lib/roundhouse/view_helpers.ex` when views emit.
const VIEW_HELPERS_SOURCE: &str =
    include_str!("../../runtime/elixir/view_helpers.ex");
/// Plug.Cowboy-based HTTP server. Ships as
/// `lib/roundhouse/server.ex`.
const SERVER_SOURCE: &str = include_str!("../../runtime/elixir/server.ex");
/// /cable stub. Ships as `lib/roundhouse/cable.ex`.
const CABLE_SOURCE: &str = include_str!("../../runtime/elixir/cable.ex");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(mix::emit_mix_exs());
    if !app.models.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse.ex"),
            content: RUNTIME_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/db.ex"),
            content: DB_SOURCE.to_string(),
        });
        files.push(schema_sql::emit_schema_sql(app));
    }
    for model in &app.models {
        files.push(model::emit_model_file(model, app));
    }
    if !app.controllers.is_empty() {
        // HTTP runtime (ActionResponse/ActionContext + Router) —
        // copied verbatim.
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/http.ex"),
            content: HTTP_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/test_support.ex"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/view_helpers.ex"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/server.ex"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/cable.ex"),
            content: CABLE_SOURCE.to_string(),
        });
        let known_models: Vec<Symbol> =
            app.models.iter().map(|m| m.name.0.clone()).collect();
        for controller in &app.controllers {
            files.push(controller::emit_controller_file_pass2(controller, &known_models, app));
        }
        files.push(route::emit_ex_route_helpers(app));
        files.push(importmap::emit_ex_importmap(app));
        files.push(main::emit_ex_main(app));
        files.push(view::emit_ex_views(app));
    }
    if !app.routes.entries.is_empty() {
        files.push(controller::emit_router_file(app));
        files.push(route::emit_ex_routes_register(app));
    }
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(fixture::emit_ex_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(fixture::emit_ex_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("test/test_helper.exs"),
            content: "ExUnit.start()\n".to_string(),
        });
        for tm in &app.test_modules {
            files.push(spec::emit_ex_test(tm, app));
        }
    }
    files
}

/// Emit a typed `MethodDef` as a standalone Elixir function (trailing
/// newline included). Elixir is dynamically typed, so the `Ty::Fn`
/// signature is used only for arity validation — param/return types
/// don't appear in the output (a future step can emit `@spec`
/// attributes for static tooling).
pub fn emit_method(m: &crate::dialect::MethodDef) -> String {
    let sig = m
        .signature
        .as_ref()
        .expect("emit_method requires a signature");
    if let crate::ty::Ty::Fn { params: sig_params, .. } = sig {
        assert_eq!(
            sig_params.len(),
            m.params.len(),
            "method `{}`: signature/param arity mismatch",
            m.name
        );
    } else {
        panic!("signature is not Ty::Fn");
    }

    let param_list: Vec<String> = m.params.iter().map(|p| p.to_string()).collect();
    let body = expr::emit_block(&m.body, None);

    let mut out = String::new();
    writeln!(out, "def {}({}) do", m.name, param_list.join(", ")).unwrap();
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("end\n");
    out
}
