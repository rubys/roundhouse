//! `roundhouse-lsp` — alias for `roundhouse lsp`: read-only Language
//! Server over the whole-app type analysis (roundhouse#57, Rung 1).
//!
//! Speaks LSP over stdio: publishes diagnostics and answers `hover` /
//! `inlayHint` with inferred types, nil-safety, and "won't-lower"
//! information that no runtime-reflection Ruby tool provides. Point any
//! LSP editor at this binary with the Rails app as the workspace root.
//!
//! The server is pure analysis — it never edits the workspace.

use std::process::ExitCode;

fn main() -> ExitCode {
    roundhouse::stack::run(|| {
        let args: Vec<String> = std::env::args().skip(1).collect();
        roundhouse::cli::lsp(&args)
    })
}
