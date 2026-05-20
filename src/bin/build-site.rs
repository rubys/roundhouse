//! `build-site` — legacy shim around `roundhouse --site`.
//!
//! The site-building logic moved to `roundhouse::project::build_site`,
//! invoked by `roundhouse --site` in the new compiler-shaped CLI.
//! This binary remains as a thin compatibility shim for the Makefile
//! and CI step that still spell the command `build-site`. Subsequent
//! work migrates those callers and deletes this file.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let fixture = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("fixtures/real-blog"));
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("_site"));

    match roundhouse::project::build_site(&fixture, &out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("build-site: {e}");
            ExitCode::FAILURE
        }
    }
}
