//! `roundhouse-mcp` — Model Context Protocol server over the whole-app
//! type analysis (roundhouse#57, Rung 2).
//!
//! Gives a coding agent the static-type feedback loop Rails lacks:
//! `type_at`, `can_be_nil`, `diagnostics`, and `wont_lower` (will this
//! survive ejection to Go/Rust/…) — no app boot, sub-second, on broken
//! code, side-effect-free.
//!
//! Speaks JSON-RPC over stdio. The Rails app root is `argv[1]`, else
//! `$ROUNDHOUSE_APP_ROOT`, else the working directory.

use std::process::ExitCode;

fn main() -> ExitCode {
    match roundhouse::mcp::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("roundhouse-mcp: fatal: {err}");
            ExitCode::FAILURE
        }
    }
}
