//! `fractal emit|apply|verify`: emit a sparse overlay, apply it to an app, or verify it.

use std::process::ExitCode;

fn main() -> ExitCode {
    roundhouse::stack::run(|| {
        let args: Vec<String> = std::env::args().skip(1).collect();
        roundhouse::cli::fractal::run(&args)
    })
}
