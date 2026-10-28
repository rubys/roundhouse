//! Separate sparse-overlay emission from applying it to an existing app,
//! and verify an applied overlay.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::analyze::Severity;
use crate::emit::fractal;
use crate::ingest::ingest_app;

const USAGE: &str = "\
Usage: fractal emit [--file PATH]... [--whole-app] [--survey] [--report PATH] [APP] [-o OUT]
       fractal apply [OVERLAY] [APP]
       fractal verify --report PATH [--ruby CMD] [--require PATH]... [--all] [--keep DIR] [OVERLAY] [APP]

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
verify Boot APP in Ruby before and after applying OVERLAY, compare what Ruby
       can observe of the compiled modules, and restore APP's files.
       --report PATH is the emit report of the build that wrote OVERLAY.
       --ruby CMD runs Ruby in APP (default: ruby), for example
       'bundle exec ruby'.
       --require PATH is required first, relative to APP (config/environment).
       --all fingerprints every named module, not only the compiled ones.
       --keep DIR keeps both fingerprints, and copies of the replaced files.
       Exits 1 when any difference, unobserved rewrite or unverified
       method is found.

The shipped roundhouse binary also accepts: roundhouse fractal emit|apply|verify.
";

#[derive(Default)]
struct Emit {
    files: Vec<String>,
    input: Option<PathBuf>,
    overlay: Option<PathBuf>,
    survey: bool,
    report: Option<PathBuf>,
    whole_app: bool,
}

enum Command {
    Help,
    Emit(Emit),
    Apply { overlay: PathBuf, app: PathBuf },
    Verify(fractal::verify::Options),
}

/// Run `fractal emit|apply|verify` or `roundhouse fractal emit|apply|verify`.
pub fn run(arguments: &[String]) -> ExitCode {
    let command = match parse(arguments) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("fractal: {error}\n\n{USAGE}");
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
            options.overlay.as_deref().unwrap_or(Path::new("out/fractal")),
            options.survey,
            options.report.as_deref(),
            options.whole_app,
        ),
        Command::Apply { overlay, app } => fractal::apply(&overlay, &app).map(|count| {
            eprintln!("fractal: applied {count} file(s) from {} to {}", overlay.display(), app.display());
        }),
        Command::Verify(options) => {
            return match fractal::verify::verify(&options) {
                Ok(outcome) => {
                    for line in outcome.render().lines() {
                        eprintln!("fractal: {line}");
                    }
                    if outcome.verified() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
                }
                Err(error) => {
                    eprintln!("fractal: {error}");
                    ExitCode::FAILURE
                }
            };
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fractal: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse(arguments: &[String]) -> Result<Command, String> {
    let Some((command, arguments)) = arguments.split_first() else {
        return Err("expected emit, apply or verify".into());
    };
    match command.as_str() {
        "-h" | "--help" => Ok(Command::Help),
        "apply" => {
            let mut paths = Vec::new();
            for argument in arguments {
                match argument.as_str() {
                    "-h" | "--help" => return Ok(Command::Help),
                    flag if flag.starts_with('-') => return Err(format!("unknown apply flag: {flag}")),
                    _ => paths.push(PathBuf::from(argument)),
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
            let mut arguments = arguments.iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "-h" | "--help" => return Ok(Command::Help),
                    "--file" => options.files.push(arguments.next().ok_or("--file requires a path")?.clone()),
                    "--whole-app" => options.whole_app = true,
                    "--survey" => options.survey = true,
                    "--report" => options.report = Some(PathBuf::from(arguments.next().ok_or("--report requires a path")?)),
                    "-o" | "--output" => options.overlay = Some(PathBuf::from(arguments.next().ok_or("--output requires a path")?)),
                    flag if flag.starts_with("--file=") => options.files.push(flag["--file=".len()..].to_string()),
                    flag if flag.starts_with("--report=") => options.report = Some(PathBuf::from(&flag["--report=".len()..])),
                    flag if flag.starts_with("--output=") => options.overlay = Some(PathBuf::from(&flag["--output=".len()..])),
                    flag if flag.starts_with('-') => return Err(format!("unknown emit flag: {flag}")),
                    _ if options.input.is_some() => return Err("emit expects at most one APP".into()),
                    _ => options.input = Some(PathBuf::from(argument)),
                }
            }
            Ok(Command::Emit(options))
        }
        "verify" => parse_verify(arguments),
        _ => Err(format!("unknown command: {command}; expected emit, apply or verify")),
    }
}

fn parse_verify(arguments: &[String]) -> Result<Command, String> {
    let mut report = None;
    let mut ruby = vec!["ruby".to_string()];
    let mut requires = Vec::new();
    let mut all = false;
    let mut keep = None;
    let mut paths = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let (flag, inline) = match argument.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (argument.as_str(), None),
        };
        let mut value = |name: &str| inline.clone().or_else(|| arguments.next().cloned()).ok_or(format!("{name} requires a value"));
        match flag {
            "-h" | "--help" => return Ok(Command::Help),
            "--report" => report = Some(PathBuf::from(value("--report")?)),
            "--ruby" => ruby = value("--ruby")?.split_whitespace().map(str::to_string).collect(),
            "--require" => requires.push(value("--require")?),
            "--keep" => keep = Some(PathBuf::from(value("--keep")?)),
            "--all" => all = true,
            flag if flag.starts_with('-') => return Err(format!("unknown verify flag: {flag}")),
            _ => paths.push(PathBuf::from(argument)),
        }
    }
    if paths.len() > 2 {
        return Err("verify expects at most OVERLAY and APP".into());
    }
    let mut paths = paths.into_iter();
    Ok(Command::Verify(fractal::verify::Options {
        overlay: paths.next().unwrap_or_else(|| PathBuf::from("out/fractal")),
        app: paths.next().unwrap_or_else(|| PathBuf::from(".")),
        report: report.ok_or("verify requires --report PATH, the emit report of the build")?,
        ruby,
        requires,
        all,
        keep,
    }))
}

/// Decide what to ingest, analyze it, and emit only the generated files.
pub fn emit(
    files: &[String],
    input: &Path,
    overlay: &Path,
    survey: bool,
    report_path: Option<&Path>,
    whole_app: bool,
) -> Result<(), String> {
    if !input.exists() {
        return Err(format!("input {} does not exist", input.display()));
    }
    // A report inside the overlay would be deleted by the next rebuild, and a
    // report named like the manifest would make an unrelated directory look
    // like an earlier build. Refuse it before reading or writing anything.
    if let Some(report_path) = report_path {
        let report_absolute = fractal::tree::absolute(report_path)?;
        if report_absolute.starts_with(fractal::tree::absolute(overlay)?) {
            return Err(format!("--report {} is inside the overlay {}; write it outside", report_path.display(), overlay.display()));
        }
    }
    let passes = fractal::pass::registered();
    let passes: Vec<&dyn fractal::Pass> = passes.iter().map(|pass| pass.as_ref()).collect();
    let root = input.canonicalize().map_err(|error| format!("input {}: {error}", input.display()))?;
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
            let filesystem = crate::vfs::FsVfs::new();
            let vfs = crate::vfs::ScopedVfs::new(&filesystem, files.iter().cloned());
            crate::ingest::app::ingest_app_with_vfs(&vfs, &root)
        }
    });
    let gaps = if survey { crate::ingest::survey::drain() } else { Vec::new() };
    let mut app = match app_result {
        Ok(app) => app,
        Err(error) => {
            for diagnostic in &parse_diags {
                eprintln!("roundhouse: {}", diagnostic.render(&[]));
            }
            if !gaps.is_empty() {
                eprintln!();
                eprint!("{}", crate::ingest::survey::render_report(&gaps));
            } else if !survey {
                eprintln!("roundhouse: hint: re-run with --survey to record unsupported constructs and keep going");
            }
            return Err(format!("ingest {}: {error}", input.display()));
        }
    };
    // A file with a syntax error or a recorded gap keeps the app's code.
    // Its IR holds placeholders, so no pass rewrites it.
    let mut untrusted: Vec<String> = Vec::new();
    for diagnostic in parse_diags.iter().filter(|diagnostic| diagnostic.severity == Severity::Error) {
        eprintln!("roundhouse: {}", diagnostic.render(&app.sources));
        if let Some(source) = (diagnostic.span.file.0 as usize).checked_sub(1).and_then(|index| app.sources.get(index)) {
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
    let written = fractal::write(input, overlay, &plan)?;
    if let Some(report_path) = report_path {
        let json = serde_json::to_string_pretty(&plan.report).map_err(|error| format!("report: {error}"))?;
        std::fs::write(report_path, json + "\n").map_err(|error| format!("write {}: {error}", report_path.display()))?;
    }
    eprintln!("roundhouse: emitted {} changed file(s) to {} (fractal overlay; every other file stays in the base app)", written.rewritten, overlay.display());
    Ok(())
}
