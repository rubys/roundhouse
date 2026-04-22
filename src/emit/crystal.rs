//! Crystal emitter.
//!
//! Last Phase 2 scaffold. Crystal is Ruby-flavored with mandatory
//! static types, which makes this the easiest scaffold of the six —
//! the emission shape is almost identical to Ruby's, plus type
//! annotations.
//!
//! Scaffold choices:
//! - Models as `class Name` with typed `property :field : Type`
//!   declarations (Crystal's getter/setter macro).
//! - Controllers as `class Name` with one method per action. No
//!   `< Kemal::Controller` base — Phase 3+ runtime work picks.
//! - Routes as a `ROUTES` constant with a NamedTuple array —
//!   Crystal's idiomatic static table shape.
//!
//! Notably not mirrored from railcar:
//! - Railcar's Crystal output uses a heavy macro DSL
//!   (`model("articles") do column(title, String) end`) to hook
//!   into its runtime. That's a Phase-4-depth choice; our scaffold
//!   stays runtime-agnostic.
//!
//! Organized into one submodule per output kind. Cross-cutting helpers
//! live in `shared`; the generic `Expr` walker lives in `expr` and is
//! reused by the model-method and controller-test emitters; type
//! rendering lives in `ty` (with `crystal_ty` re-exported here for the
//! external surface that `bin/build-site` uses).

use std::path::PathBuf;

use super::EmittedFile;
use crate::App;

mod app;
mod controller;
mod controller_test;
mod expr;
mod fixture;
mod importmap;
mod main;
mod model;
mod route;
mod schema_sql;
mod shard;
mod shared;
mod spec;
mod ty;
mod view;

// External API: kept for `bin/build-site` and tests that key off
// `crystal_ty` directly.
pub use ty::crystal_ty;

const RUNTIME_SOURCE: &str = include_str!("../../runtime/crystal/runtime.cr");
const DB_SOURCE: &str = include_str!("../../runtime/crystal/db.cr");
/// Crystal HTTP runtime — ActionResponse/ActionContext + in-memory
/// Router match table. Mirrors `runtime/rust/http.rs` +
/// `runtime/typescript/juntos.ts`; emitted controllers register
/// handlers through Router.add + tests dispatch via Router.match.
const HTTP_SOURCE: &str = include_str!("../../runtime/crystal/http.cr");
/// Crystal test-support runtime — TestClient + TestResponse with
/// Rails-shaped assertions (assert_ok, assert_redirected_to,
/// assert_select, etc). Dispatches through Router.match, no real HTTP.
const TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/crystal/test_support.cr");
/// Crystal view helpers — link_to/button_to/form_wrap/FormBuilder.
/// Minimal HTML-returning surface covering the scaffold blog's ERB
/// uses; substring-match assertions in controller specs pass with
/// this level of fidelity.
const VIEW_HELPERS_SOURCE: &str = include_str!("../../runtime/crystal/view_helpers.cr");
/// Crystal HTTP::Server runtime — `Roundhouse::Server.start`
/// dispatches through Router.match, wraps HTML in the emitted
/// layout, and handles `_method` override. Copied as `src/server.cr`.
const SERVER_SOURCE: &str = include_str!("../../runtime/crystal/server.cr");
/// Crystal cable stub — `/cable` handler returning 426. Copied as
/// `src/cable.cr`.
const CABLE_SOURCE: &str = include_str!("../../runtime/crystal/cable.cr");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(shard::emit_shard_yml());
    if !app.models.is_empty() {
        files.push(model::emit_models(app));
        // Runtime tags along whenever any model is emitted — validate()
        // calls ValidationError.new.
        files.push(EmittedFile {
            path: PathBuf::from("src/runtime.cr"),
            content: RUNTIME_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/db.cr"),
            content: DB_SOURCE.to_string(),
        });
        files.push(schema_sql::emit_schema_sql(app));
    }
    if !app.controllers.is_empty() {
        // HTTP runtime — copied verbatim, same posture as `runtime.cr`
        // / `db.cr`. Provides the `Roundhouse::Http` surface that
        // emitted controller actions call into.
        files.push(EmittedFile {
            path: PathBuf::from("src/http.cr"),
            content: HTTP_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/view_helpers.cr"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/test_support.cr"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/server.cr"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/cable.cr"),
            content: CABLE_SOURCE.to_string(),
        });
        files.push(controller::emit_controllers(app));
        files.extend(view::emit_views_cr(app));
        files.push(route::emit_route_helpers_cr(app));
        files.push(importmap::emit_cr_importmap(app));
        files.push(main::emit_cr_main(app));
    }
    if !app.routes.entries.is_empty() {
        files.push(route::emit_routes(app));
    }
    // Fixtures as modules under spec/fixtures/. Emitted as individual
    // files plus a top-level spec/fixtures.cr helper.
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(fixture::emit_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(fixture::emit_crystal_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(spec::emit_crystal_spec(tm, app));
        }
        files.push(spec::emit_spec_helper(app));
    }
    files.push(app::emit_app_cr(app));
    files
}
