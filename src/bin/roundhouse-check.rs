//! `roundhouse-check` — alias for `roundhouse check`: run analyze +
//! diagnose on a Rails app and print the diagnostics. Exit zero if
//! empty, one if not. See `roundhouse::cli::check`.
//!
//! Usage:
//!
//!     cargo run --bin roundhouse-check -- [--continue] [FIXTURE]
//!
//! Default FIXTURE is `fixtures/real-blog` (the multi-call
//! `roundhouse check` defaults to the working directory instead).

use std::process::ExitCode;

fn main() -> ExitCode {
    roundhouse::stack::run(|| {
        let args: Vec<String> = std::env::args().skip(1).collect();
        roundhouse::cli::check(&args, "fixtures/real-blog")
    })
}
