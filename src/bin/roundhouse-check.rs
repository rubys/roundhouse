//! `roundhouse-check` — run analyze + diagnose on a Rails app and
//! print the diagnostics. Exit zero if empty, one if not.
//!
//! This is the first user-facing path for roundhouse's typed IR
//! diagnostics. It's the "did my Ruby type cleanly?" check: point
//! it at a fixture or a real Rails app, get back a list of sites
//! the analyzer flagged (unresolved ivars, method dispatch failures,
//! incompatible operator uses).
//!
//! Today's output is message-only — spans are not yet resolvable to
//! file:line:column. Identifier names in the messages are the user's
//! grep targets until real span infrastructure lands (tracked
//! separately; see blog post 3416 for the sketch).
//!
//! Usage:
//!
//!     cargo run --bin roundhouse-check -- [FIXTURE]
//!
//! Default FIXTURE is `fixtures/real-blog`.

use std::path::Path;
use std::process::ExitCode;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app;

fn main() -> ExitCode {
    let fixture = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fixtures/real-blog".into());
    let path = Path::new(&fixture);

    let mut app = match ingest_app(path) {
        Ok(app) => app,
        Err(err) => {
            eprintln!("roundhouse-check: ingest failed: {err}");
            return ExitCode::from(2);
        }
    };
    Analyzer::new(&app).analyze(&mut app);
    let diags = diagnose(&app);

    if diags.is_empty() {
        eprintln!("roundhouse-check: {} — 0 diagnostics", fixture);
        return ExitCode::SUCCESS;
    }

    for d in &diags {
        eprintln!("{d}");
    }
    eprintln!();
    eprintln!(
        "roundhouse-check: {} — {} diagnostic(s)",
        fixture,
        diags.len()
    );
    ExitCode::FAILURE
}
