//! `roundhouse-lsp` — read-only Language Server over the whole-app type
//! analysis (roundhouse#57, Rung 1).
//!
//! Speaks LSP over stdio: publishes diagnostics and answers `hover` /
//! `inlayHint` with inferred types, nil-safety, and "won't-lower"
//! information that no runtime-reflection Ruby tool provides. Point any
//! LSP editor at this binary with the Rails app as the workspace root.
//!
//! The server is pure analysis — it never edits the workspace.

use std::process::ExitCode;

fn main() -> ExitCode {
    match roundhouse::lsp::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("roundhouse-lsp: fatal: {err}");
            ExitCode::FAILURE
        }
    }
}
