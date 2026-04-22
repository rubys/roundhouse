//! `go.mod` + `go.sum` emission — Go's project config files.

use std::path::PathBuf;

use super::super::EmittedFile;

/// `go.mod` — module name `app`, targeting Go 1.24. Depends on
/// modernc.org/sqlite for the pure-Go SQLite driver that the Phase 3
/// persistence runtime uses. The toolchain test runs `go mod tidy`
/// before building to populate go.sum.
pub(super) fn emit_go_mod() -> EmittedFile {
    // nhooyr.io/websocket powers /cable; go mod tidy resolves its
    // transitive graph on first build.
    let content = "module app\n\ngo 1.24\n\nrequire (\n\tmodernc.org/sqlite v1.34.1\n\tnhooyr.io/websocket v1.8.10\n)\n";
    EmittedFile {
        path: PathBuf::from("go.mod"),
        content: content.to_string(),
    }
}

/// Empty `go.sum` placeholder — `go mod tidy` populates it on first
/// build. Emitting the file up front avoids a chicken-and-egg where
/// `go vet` refuses to run without it.
pub(super) fn emit_go_sum() -> EmittedFile {
    EmittedFile {
        path: PathBuf::from("go.sum"),
        content: String::new(),
    }
}
