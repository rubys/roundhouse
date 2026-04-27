//! TypeScript emitter — targets Juntos.
//!
//! Started as the Phase 2 scaffold; Phase 3 upgrades the output to
//! Juntos's runtime shape (the committed TS target per the strategy
//! memory). Changes land incrementally so each commit stays small:
//!
//! - **Models** (this commit): `extends ApplicationRecord`, `static
//!   table_name`, `static columns`. One file per model under
//!   `app/models/<snake>.ts`. Schema-derived instance fields drop
//!   from the class body — Juntos materializes them at runtime from
//!   the `columns` list, and declaring them statically would
//!   collide with the runtime accessors.
//! - Validations, associations, broadcasts: separate Phase 3 commits
//!   once this first shape is in place.
//! - Controllers + router + views: later Phase 3 commits.
//!
//! Ruby → Juntos translation rules come from ruby2js's
//! `lib/ruby2js/filter/rails/model.rb` and `lib/ruby2js/filter/rails/
//! active_record.rb`. Those are the reference; our job is to produce
//! equivalent output driven by the typed IR.
//!
//! Non-goals still (later Phase 3 commits):
//! - Controller shape (extends Controller, ivar-style state).
//! - Router emit (Router.resources calls, not a flat table).
//! - View / template emission.
//! - `tsc --strict` cleanliness.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::ty::Ty;

mod controller;
mod expr;
mod fixture;
mod library;
mod model;
mod model_from_library;
mod naming;
mod package_json;
mod route;
mod schema_sql;
mod spec;
mod ty;
mod view;

pub use ty::ts_ty;

/// Hand-written Juntos-shape stub, copied into every generated project
/// as `src/juntos.ts`. tsconfig's `paths` alias rewrites `"juntos"`
/// imports to this file for type-checking without requiring npm
/// install. Real deployments swap in the actual Juntos package.
const JUNTOS_STUB_SOURCE: &str = include_str!("../../runtime/typescript/juntos.ts");

/// TypeScript HTTP runtime — Phase 4c compile-only stubs. Copied
/// verbatim into generated projects as `src/http.ts` when any
/// controller emits. Mirrors the Rust/Crystal/Go/Elixir twins.
const HTTP_STUB_SOURCE: &str = include_str!("../../runtime/typescript/http.ts");

/// Pass-2 test-support runtime. `TestClient` + `TestResponse` with
/// assertion methods mirroring Rust's `TestResponseExt` trait.
const TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/typescript/test_support.ts");

/// Pass-2 view-helpers runtime. Rails-compatible `linkTo`,
/// `buttonTo`, `formWrap`, `FormBuilder`, `turboStreamFrom`, etc.
const VIEW_HELPERS_SOURCE: &str = include_str!("../../runtime/typescript/view_helpers.ts");

/// HTTP + Action Cable server runtime. Copied into generated
/// projects as `src/server.ts`. Consumed by `main.ts` to start
/// the HTTP listener + WebSocket upgrade handler.
const SERVER_SOURCE: &str = include_str!("../../runtime/typescript/server.ts");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    // Default adapter for backward-compatible callers. Matches
    // pre-adapter-consumption behavior — nothing suspends, no
    // awaits beyond the `async function` wrapper that was already
    // there.
    emit_with_adapter(app, &crate::adapter::SqliteAdapter)
}

/// Emit library-shape TypeScript — for transpiled-shape input where
/// model bodies contain explicit getters/setters/lifecycle hooks
/// rather than class-level Rails DSLs. Complementary to `emit`;
/// skips the Rails-app-shaped artifacts (controllers, routes, views,
/// fixtures, test specs, HTTP/server runtime) and emits only the
/// package scaffold + juntos stub + one TS file per model.
///
/// Intended entry point for emitting framework Ruby (the forthcoming
/// Ruby-authored ActiveRecord runtime) and any other library-shape
/// input whose job is "produce importable TS classes," not "produce
/// a runnable Rails app."
pub fn emit_library(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(package_json::emit_package_json());
    files.push(package_json::emit_tsconfig_json(app));
    files.push(EmittedFile {
        path: PathBuf::from("src/juntos.ts"),
        content: JUNTOS_STUB_SOURCE.to_string(),
    });
    files.extend(library::emit_library_classes(app));
    files.extend(library::emit_library_class_decls(app));
    files
}

/// Emit a typed `MethodDef` as a standalone exported TypeScript
/// function (trailing newline included). Requires `signature` to be
/// populated — `parse_methods_with_rbs` does this. Used by the
/// runtime-extraction pipeline.
pub fn emit_method(m: &crate::dialect::MethodDef) -> String {
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
        .map(|(name, p)| format!("{}: {}", name, ts_ty(&p.ty)))
        .collect();

    let ret_s = ts_ty(ret);
    let body = expr::emit_body(&m.body, ret);

    let mut out = String::new();
    writeln!(
        out,
        "export function {}({}): {} {{",
        m.name,
        param_list.join(", "),
        ret_s
    )
    .unwrap();
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("}\n");
    out
}

/// Emit with an explicit adapter. Async-capable targets (this one,
/// eventually Rust and Python) consult the adapter's
/// `is_suspending_effect` per Send site and insert `await` where
/// effects suspend. `SqliteAdapter` suspends nothing; `SqliteAsync
/// Adapter` suspends on DB effects — emit a fully-awaited body
/// that can later be pointed at a real async backend (IndexedDB,
/// D1, pg-on-Node) without further emitter changes.
pub fn emit_with_adapter(
    app: &App,
    adapter: &dyn crate::adapter::DatabaseAdapter,
) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(package_json::emit_package_json());
    files.push(package_json::emit_tsconfig_json(app));
    files.push(EmittedFile {
        path: PathBuf::from("src/juntos.ts"),
        content: JUNTOS_STUB_SOURCE.to_string(),
    });
    if !app.models.is_empty() {
        files.push(schema_sql::emit_schema_sql(app));
    }
    files.extend(model::emit_models(app));
    files.extend(library::emit_library_class_decls(app));
    if !app.controllers.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("src/http.ts"),
            content: HTTP_STUB_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/test_support.ts"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/view_helpers.ts"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/server.ts"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(controller::emit_ts_route_helpers(app));
        // Always emit `src/importmap.ts` — empty PINS list when
        // the app has no `config/importmap.rb` — so the layout's
        // import line never fails to resolve.
        files.push(controller::emit_ts_importmap(app));
        files.extend(controller::emit_controllers(app, adapter));
        // Note: db/seeds.ts emission deferred. The top-level Ruby
        // transpile path (needed for seeds.rb → runnable TS)
        // requires more careful handling than the controller-body
        // emitter provides today: operator methods (`==` → `===`),
        // bang-stripping on class methods (`Article.create!` →
        // `Article.create`), and statement-structure preservation
        // through nested `if`/`unless` guards. See App::seeds for
        // the ingested expression; Ruby emit round-trips
        // correctly. TS emission picks up in a later bite.
        files.push(controller::emit_main_ts(app));
    }
    files.extend(view::emit_views(app));
    if !app.routes.entries.is_empty() {
        files.push(route::emit_routes(app));
    }
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(fixture::emit_ts_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(fixture::emit_ts_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(spec::emit_ts_spec(tm, app));
        }
    }
    files
}
