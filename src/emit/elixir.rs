//! Elixir emitter.
//!
//! As of Phase D the Elixir output is produced entirely by the elixir2
//! (`V2.*`) lowered-IR overlay (`src/emit/elixir2/`). This module is the
//! thin shim that wraps it: it emits the shared, target-infrastructure
//! files the v2 stack depends on — the `mix.exs` project, the hand-written
//! `Roundhouse.Db` connection pool (`V2.Db` wraps it), the generated
//! `Roundhouse.SchemaSQL` DDL, and the ExUnit `test_helper.exs` — then
//! delegates every app module (models, views, controllers, router,
//! dispatch, server, main, tests) to `elixir2::emit_overlay_files`.
//!
//! The legacy v1 app-shell emitters (controller/model/view/spec/… and
//! their hand-written runtime `.ex` files) were retired in Phase D3; their
//! coverage now lives in the v2 emit + its `mix test` gate.

use std::path::PathBuf;

use super::EmittedFile;
use crate::App;

mod mix;
mod schema_sql;

/// Hand-written SQLite connection pool (`Roundhouse.Db`). Shared
/// target-runtime kept across the strangler phase: `V2.Db` wraps it, and
/// `V2.Server` / the v2 fixtures open the DB through it. Copied verbatim
/// into the generated project as `lib/roundhouse/db.ex`.
const DB_SOURCE: &str = include_str!("../../runtime/elixir/db.ex");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(mix::emit_mix_exs());
    if !app.models.is_empty() {
        // `Roundhouse.Db` (connection pool) + `Roundhouse.SchemaSQL` (DDL)
        // — referenced by the v2 model adapters, `V2.Db`, `V2.Server`, and
        // the v2 fixtures.
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/db.ex"),
            content: DB_SOURCE.to_string(),
        });
        files.push(schema_sql::emit_schema_sql(app));
    }
    if !app.test_modules.is_empty() {
        // Shared ExUnit entry point; the v2 test tree (test/v2/**) loads it.
        files.push(EmittedFile {
            path: PathBuf::from("test/test_helper.exs"),
            content: "ExUnit.start()\n".to_string(),
        });
    }
    // The v2 (`V2.*`) overlay: every app module + its runtime.
    files.extend(super::elixir2::emit_overlay_files(app));
    files
}
