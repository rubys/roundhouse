//! `roundhouse` — compile a Rails source application to a target
//! language. Compiler-shaped CLI: positional input, `-o` output,
//! flags for everything else.
//!
//! Two modes:
//!
//!   `roundhouse --target LANG [INPUT] [-o OUT]`
//!       Single-target transpile. Default INPUT=`.`, default
//!       OUT=`./out/<lang>/`. Writes the emitted file set to OUT
//!       and (unless the emit already includes one) a quick-start
//!       README at OUT/README.md.
//!
//!   `roundhouse --site [INPUT] [-o OUT]`
//!       Build the full GitHub Pages site: per-target archives
//!       (`<lang>.json`, `.tgz`, `.zip`) plus static landing-page
//!       assets. Default INPUT=`fixtures/real-blog`, default
//!       OUT=`./_site/`.
//!
//! `--target` and `--site` are mutually exclusive; exactly one must
//! be specified. See `--help` for the full flag list.

use std::path::PathBuf;
use std::process::ExitCode;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app;
use roundhouse::project::{self, BuildTarget};

fn usage() -> &'static str {
    "\
Usage: roundhouse --target LANG [INPUT] [-o OUT]
       roundhouse --site [INPUT] [-o OUT]
       roundhouse --help | --version

Transpile a Rails source application to a target language, or build
the multi-target Pages site.

Options:
  -t, --target LANG    Transpile target. One of:
                         crystal, elixir, go, python, rust,
                         typescript, typescript-worker, ruby, spinel
                       Default INPUT=.  Default OUT=./out/<lang>/
      --site           Build all targets + landing-page assets.
                       Default INPUT=fixtures/real-blog  Default OUT=./_site/
  -o, --output PATH    Output directory.
  -h, --help           Show this help and exit.
  -V, --version        Show version and exit.

Examples:
  roundhouse --target rust                            # → ./out/rust/
  roundhouse --target typescript -o build/ my-app/    # explicit input + output
  roundhouse --site                                   # → ./_site/
"
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1).collect()) {
        Ok(Action::Help) => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Ok(Action::Version) => {
            println!("roundhouse {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Action::Transpile { target, input, out }) => match run_transpile(target, &input, &out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("roundhouse: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(Action::Site { input, out }) => match project::build_site(&input, &out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("roundhouse: {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("roundhouse: {e}");
            eprintln!();
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

enum Action {
    Help,
    Version,
    Transpile {
        target: BuildTarget,
        input: PathBuf,
        out: PathBuf,
    },
    Site {
        input: PathBuf,
        out: PathBuf,
    },
}

fn parse_args(args: Vec<String>) -> Result<Action, String> {
    let mut target: Option<BuildTarget> = None;
    let mut site = false;
    let mut out: Option<PathBuf> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Action::Help),
            "-V" | "--version" => return Ok(Action::Version),
            "--site" => site = true,
            "-t" | "--target" => {
                let v = iter
                    .next()
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                target = Some(parse_target(&v)?);
            }
            "-o" | "--output" => {
                let v = iter
                    .next()
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                out = Some(PathBuf::from(v));
            }
            s if s.starts_with("--target=") => {
                target = Some(parse_target(&s["--target=".len()..])?);
            }
            s if s.starts_with("--output=") => {
                out = Some(PathBuf::from(&s["--output=".len()..]));
            }
            s if s.starts_with('-') => {
                return Err(format!("unknown flag: {s}"));
            }
            _ => positional.push(arg),
        }
    }

    if positional.len() > 1 {
        return Err(format!(
            "expected at most one positional INPUT, got {}: {}",
            positional.len(),
            positional.join(" ")
        ));
    }

    match (target, site) {
        (Some(_), true) => Err("--target and --site are mutually exclusive".into()),
        (None, false) => Err("one of --target LANG or --site is required".into()),
        (Some(target), false) => {
            let input = positional
                .pop()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let out =
                out.unwrap_or_else(|| PathBuf::from("out").join(target.as_str()));
            Ok(Action::Transpile { target, input, out })
        }
        (None, true) => {
            let input = positional
                .pop()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("fixtures/real-blog"));
            let out = out.unwrap_or_else(|| PathBuf::from("_site"));
            Ok(Action::Site { input, out })
        }
    }
}

fn parse_target(s: &str) -> Result<BuildTarget, String> {
    match BuildTarget::from_str(s) {
        Some(BuildTarget::Blog) => Err(format!(
            "--target blog is not a transpile target (use --site to include the source archive)"
        )),
        Some(t) => Ok(t),
        None => {
            let valid = BuildTarget::TRANSPILE
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!("unknown target '{s}'. Valid: {valid}"))
        }
    }
}

fn run_transpile(
    target: BuildTarget,
    input: &std::path::Path,
    out: &std::path::Path,
) -> Result<(), String> {
    if !input.exists() {
        return Err(format!("input {} does not exist", input.display()));
    }
    let mut app =
        ingest_app(input).map_err(|e| format!("ingest {}: {e}", input.display()))?;
    Analyzer::new(&app).analyze(&mut app);

    let mut files = project::target_files(&app, input, target)?;
    let has_readme = files.iter().any(|(p, _)| p == "README.md");
    if !has_readme {
        files.push(("README.md".to_string(), project::target_readme(target)));
    }

    project::write_to_dir(&files, out)?;
    eprintln!(
        "roundhouse: wrote {} files to {} ({})",
        files.len(),
        out.display(),
        target.as_str()
    );
    Ok(())
}
