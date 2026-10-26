//! `fractal emit|apply`: emit a sparse overlay, then apply it to an app.

use std::process::ExitCode;

fn main() -> ExitCode {
    roundhouse::stack::run(|| {
        let args: Vec<String> = std::env::args().skip(1).collect();
        roundhouse::cli::fractal::run(&args)
    })
}
