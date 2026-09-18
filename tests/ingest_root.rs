//! `ingest_app` at the library boundary: a root that is not a
//! directory is an error naming the path, never an empty app. The
//! `check` binary guards its own argument (`tests/cli_check.rs`); this
//! is the guard every other caller — the LSP and MCP servers, a test
//! against a fixture nobody generated — inherits.

use std::path::Path;

use roundhouse::ingest::ingest_app;

#[test]
fn a_missing_root_is_an_error_not_an_empty_app() {
    let err = ingest_app(Path::new("/nonexistent/rails/app"))
        .err()
        .expect("a missing root must not ingest");
    let msg = err.to_string();
    assert!(msg.contains("/nonexistent/rails/app is not a directory"), "{msg}");
}

#[test]
fn a_file_root_is_an_error_not_an_empty_app() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let err = ingest_app(&manifest).err().expect("a file root must not ingest");
    let msg = err.to_string();
    assert!(msg.contains("Cargo.toml is not a directory"), "{msg}");
}
