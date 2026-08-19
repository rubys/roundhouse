//! Elixir emit entry — the public `emit`: the shared
//! target-infrastructure files the `V2.*` overlay depends on (the
//! `mix.exs` project, the hand-written `Roundhouse.Db` connection
//! pool that `V2.Db` wraps, the generated `Roundhouse.SchemaSQL` DDL,
//! the ExUnit `test_helper.exs`), then every app module via
//! `emit_overlay_files`. Lived at the old `src/emit/elixir.rs` shim
//! until the elixir2→elixir rename folded it in here.

use std::path::PathBuf;

use crate::App;
use crate::emit::EmittedFile;

use super::{mix, schema_sql};

/// Hand-written SQLite connection pool (`Roundhouse.Db`). Shared
/// target-runtime kept across the strangler phase: `V2.Db` wraps it, and
/// `V2.Server` / the v2 fixtures open the DB through it. Copied verbatim
/// into the generated project as `lib/roundhouse/db.ex`.
const DB_SOURCE: &str = include_str!("../../../runtime/elixir/db.ex");

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
    files.extend(super::emit_overlay_files(app));
    files
}
