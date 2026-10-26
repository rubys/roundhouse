//! Separate sparse-overlay emission from applying it to an existing app.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::analyze::Severity;
use crate::emit::fractal;
use crate::ingest::ingest_app;

const USAGE: &str = "\
Usage: fractal emit [--file PATH]... [--whole-app] [--survey] [--report PATH] [APP] [-o OUT]
       fractal apply [OVERLAY] [APP]

emit   Write only changed files in the app's directory structure.
       APP defaults to .; OUT defaults to out/fractal.
       --file PATH selects an app-relative Ruby file; repeat it for more files.
       --whole-app analyzes the whole app instead of the selected files.
       --survey records unsupported input instead of stopping at the first gap.
       --report PATH writes the build decisions as JSON outside the overlay.
       The overlay includes a build manifest; apply does not copy it to APP.
apply  Copy overlay files onto APP, replacing matching files and adding new files.
       OVERLAY defaults to out/fractal; APP defaults to .
       Files absent from the overlay stay unchanged. Apply never deletes files.
       Source and destination entries must not be symlinks.

The shipped roundhouse binary also accepts: roundhouse fractal emit|apply.
";

#[derive(Default)]
struct Emit {
    files: Vec<String>,
    input: Option<PathBuf>,
    out: Option<PathBuf>,
    survey: bool,
    report: Option<PathBuf>,
    whole_app: bool,
}

enum Command {
    Help,
    Emit(Emit),
    Apply { overlay: PathBuf, app: PathBuf },
}

/// Run `fractal emit|apply` or `roundhouse fractal emit|apply`.
pub fn run(args: &[String]) -> ExitCode {
    let command = match parse(args) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("fractal: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Help => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Command::Emit(options) => emit(
            &options.files,
            options.input.as_deref().unwrap_or(Path::new(".")),
            options.out.as_deref().unwrap_or(Path::new("out/fractal")),
            options.survey,
            options.report.as_deref(),
            options.whole_app,
        ),
        Command::Apply { overlay, app } => fractal::apply(&overlay, &app).map(|count| {
            eprintln!("fractal: applied {count} file(s) from {} to {}", overlay.display(), app.display());
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("fractal: {err}");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[String]) -> Result<Command, String> {
    let Some((command, args)) = args.split_first() else {
        return Err("expected emit or apply".into());
    };
    match command.as_str() {
        "-h" | "--help" => Ok(Command::Help),
        "apply" => {
            let mut paths = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "-h" | "--help" => return Ok(Command::Help),
                    s if s.starts_with('-') => return Err(format!("unknown apply flag: {s}")),
                    _ => paths.push(PathBuf::from(arg)),
                }
            }
            if paths.len() > 2 {
                return Err("apply expects at most OVERLAY and APP".into());
            }
            let mut paths = paths.into_iter();
            Ok(Command::Apply {
                overlay: paths.next().unwrap_or_else(|| PathBuf::from("out/fractal")),
                app: paths.next().unwrap_or_else(|| PathBuf::from(".")),
            })
        }
        "emit" => {
            let mut options = Emit::default();
            let mut args = args.iter();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-h" | "--help" => return Ok(Command::Help),
                    "--file" => options.files.push(args.next().ok_or("--file requires a path")?.clone()),
                    "--whole-app" => options.whole_app = true,
                    "--survey" => options.survey = true,
                    "--report" => options.report = Some(PathBuf::from(args.next().ok_or("--report requires a path")?)),
                    "-o" | "--output" => options.out = Some(PathBuf::from(args.next().ok_or("--output requires a path")?)),
                    s if s.starts_with("--file=") => options.files.push(s["--file=".len()..].to_string()),
                    s if s.starts_with("--report=") => options.report = Some(PathBuf::from(&s["--report=".len()..])),
                    s if s.starts_with("--output=") => options.out = Some(PathBuf::from(&s["--output=".len()..])),
                    s if s.starts_with('-') => return Err(format!("unknown emit flag: {s}")),
                    _ if options.input.is_some() => return Err("emit expects at most one APP".into()),
                    _ => options.input = Some(PathBuf::from(arg)),
                }
            }
            Ok(Command::Emit(options))
        }
        _ => Err(format!("unknown command: {command}; expected emit or apply")),
    }
}

/// Decide what to ingest, analyze it, and emit only the generated files.
pub fn emit(
    files: &[String],
    input: &Path,
    out: &Path,
    survey: bool,
    report_path: Option<&Path>,
    whole_app: bool,
) -> Result<(), String> {
    if !input.exists() {
        return Err(format!("input {} does not exist", input.display()));
    }
    let passes = fractal::pass::registered();
    let passes: Vec<&dyn fractal::Pass> = passes.iter().map(|p| p.as_ref()).collect();
    let root = input.canonicalize().map_err(|e| format!("input {}: {e}", input.display()))?;
    let files = fractal::scope::normalize_files(&root, files)?;
    let ingest = fractal::scope::decide(&root, &files, &passes, whole_app)?;
    if survey {
        crate::ingest::survey::activate();
    }
    let (app_result, parse_diags) = crate::ingest::prism::scope(|| match &ingest {
        fractal::Ingest::WholeApp { reason } => {
            eprintln!("roundhouse: fractal: ingesting the whole app ({reason})");
            ingest_app(input)
        }
        fractal::Ingest::Files { files, indexed } => {
            eprintln!(
                "roundhouse: fractal: ingesting {} file(s) the compiled files and passes need (Rubydex indexed {indexed}); --whole-app reads everything",
                files.len()
            );
            let fs = crate::vfs::FsVfs::new();
            let vfs = crate::vfs::ScopedVfs::new(&fs, files.iter().cloned());
            crate::ingest::app::ingest_app_with_vfs(&vfs, &root)
        }
    });
    let gaps = if survey { crate::ingest::survey::drain() } else { Vec::new() };
    let mut app = match app_result {
        Ok(app) => app,
        Err(e) => {
            for d in &parse_diags {
                eprintln!("roundhouse: {}", d.render(&[]));
            }
            if !gaps.is_empty() {
                eprintln!();
                eprint!("{}", crate::ingest::survey::render_report(&gaps));
            } else if !survey {
                eprintln!("roundhouse: hint: re-run with --survey to record unsupported constructs and keep going");
            }
            return Err(format!("ingest {}: {e}", input.display()));
        }
    };
    // A file with a syntax error or a recorded gap keeps the app's code.
    // Its IR holds placeholders, so no pass rewrites it.
    let mut untrusted: Vec<String> = Vec::new();
    for d in parse_diags.iter().filter(|d| d.severity == Severity::Error) {
        eprintln!("roundhouse: {}", d.render(&app.sources));
        if let Some(source) = (d.span.file.0 as usize).checked_sub(1).and_then(|i| app.sources.get(i)) {
            untrusted.push(source.path.clone());
        }
    }
    if !gaps.is_empty() {
        eprintln!();
        eprint!("{}", crate::ingest::survey::render_report(&gaps));
        eprintln!("roundhouse: {} ingest gap(s) recorded; their files keep the app's code", gaps.len());
        for gap in &gaps {
            if let crate::ingest::IngestError::Unsupported { file, .. } | crate::ingest::IngestError::Parse { file, .. } = gap {
                untrusted.push(file.clone());
            }
        }
    }
    crate::analyze::Analyzer::new(&app).analyze(&mut app);
    let plan = fractal::plan(&app, input, &files, &untrusted, &passes)?;
    for line in plan.report.render().lines() {
        eprintln!("roundhouse: {line}");
    }
    if let Some(path) = report_path {
        let json = serde_json::to_string_pretty(&plan.report).map_err(|e| format!("report: {e}"))?;
        std::fs::write(path, json + "\n").map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    let written = fractal::write(input, out, &plan)?;
    eprintln!("roundhouse: emitted {} changed file(s) to {} (fractal overlay; every other file stays in the base app)", written.rewritten, out.display());
    Ok(())
}
