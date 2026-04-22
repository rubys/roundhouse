//! `shard.yml` emission — Crystal's equivalent of `Cargo.toml`.

use std::path::PathBuf;

use super::super::EmittedFile;

/// Minimal `shard.yml` — Crystal's equivalent of `Cargo.toml`. Declares
/// one named `targets` entry so `crystal build` knows the entry point;
/// the entry file (`src/app.cr`) requires whatever modules we emitted.
pub(super) fn emit_shard_yml() -> EmittedFile {
    let content = "\
name: app
version: 0.1.0

targets:
  app:
    main: src/main.cr

dependencies:
  sqlite3:
    github: crystal-lang/crystal-sqlite3
    version: ~> 0.21
";
    EmittedFile {
        path: PathBuf::from("shard.yml"),
        content: content.to_string(),
    }
}
