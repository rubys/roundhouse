//! `roundhouse` — compile a Rails source application to a target
//! language. Compiler-shaped CLI: positional input, `-o` output,
//! flags for everything else.
//!
//! Two modes:
//!
//!   `roundhouse --target LANG [INPUT] [-o OUT]`
//!       Single-target transpile. Default INPUT=`.`, default
//!       OUT=`./out/<lang>/`. Writes the emitted file set to OUT;
//!       the set includes a quick-start README (injected by
//!       `project::target_files` unless the target ships its own).
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

use roundhouse::analyze::{diagnose, Severity};
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
                         crystal, csharp, elixir, go, kotlin, python, rust,
                         swift, typescript, typescript-worker, ruby, jruby,
                         spinel
                       Default INPUT=.  Default OUT=./out/<lang>/
      --site           Build all targets + landing-page assets.
                       Default INPUT=fixtures/real-blog  Default OUT=./_site/
  -o, --output PATH    Output directory.
      --allow-unsupported
                       Don't fail on unsupported-construct gaps: emit a
                       stub at each site, downgrade the diagnostics to
                       warnings, and write the output anyway. Use to see
                       the full inventory of gaps in one run.
      --survey         Don't abort INGEST at the first unsupported
                       construct: record it as a gap, substitute a nil
                       placeholder, and keep going. Prints a
                       deduplicated punch list, and downgrades the
                       diagnostics those gaps caused to notes. Combine
                       with --allow-unsupported to transpile an app that
                       is not fully covered yet.
  -h, --help           Show this help and exit.
  -V, --version        Show version and exit.

Examples:
  roundhouse --target rust                            # → ./out/rust/
  roundhouse --target typescript -o build/ my-app/    # explicit input + output
  roundhouse --site                                   # → ./_site/
  roundhouse -t ruby --survey --allow-unsupported ~/git/mastodon
                                                      # partly-covered app
"
}

fn main() -> ExitCode {
    roundhouse::stack::run(cli)
}

fn cli() -> ExitCode {
    match parse_args(std::env::args().skip(1).collect()) {
        Ok(Action::Help) => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Ok(Action::Version) => {
            println!("roundhouse {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Action::Transpile { target, input, out, allow_unsupported, survey }) => {
            match run_transpile(target, &input, &out, allow_unsupported, survey) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("roundhouse: {e}");
                    ExitCode::FAILURE
                }
            }
        }
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
        allow_unsupported: bool,
        survey: bool,
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
    let mut allow_unsupported = false;
    let mut survey = false;
    let mut positional: Vec<String> = Vec::new();

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Action::Help),
            "-V" | "--version" => return Ok(Action::Version),
            "--site" => site = true,
            "--allow-unsupported" => allow_unsupported = true,
            "--survey" => survey = true,
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
            Ok(Action::Transpile { target, input, out, allow_unsupported, survey })
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
    allow_unsupported: bool,
    survey: bool,
) -> Result<(), String> {
    if !input.exists() {
        return Err(format!("input {} does not exist", input.display()));
    }
    // Survey mode: record unsupported constructs as gaps and substitute
    // a nil placeholder instead of aborting at the first one. Without
    // it the ingester is fail-fast, which is right for a supported app
    // and useless for an unsupported one — mastodon dies on a single
    // `alias` in `account.rb` after 0.02s, having transpiled nothing.
    // The /ide/ page has always done this (its wasm entry calls
    // `survey::activate()`), so before this flag the BROWSER could
    // transpile an app the command line could not.
    if survey {
        roundhouse::ingest::survey::activate();
    }
    // Ingest inside a parse-diagnostic scope so Prism syntax errors —
    // which the error-recovering parser otherwise drops on the floor —
    // are collected and routed through the same report as emit gaps.
    let (app_result, parse_diags) =
        roundhouse::ingest::prism::scope(|| ingest_app(input));
    let survey_gaps =
        if survey { roundhouse::ingest::survey::drain() } else { Vec::new() };
    let mut app = match app_result {
        Ok(app) => app,
        Err(e) => {
            // Ingest failed outright. Surface any syntax errors first: a
            // malformed file is usually the root cause of the construct
            // ingest then choked on. The sources table isn't populated on
            // the error path, so these render message-only.
            for d in &parse_diags {
                eprintln!("roundhouse: {}", d.render(&[]));
            }
            // Survey mode can still fail — on I/O, or on a construct
            // outside `ingest_expr`'s interception. Print what it did
            // collect rather than dropping it.
            if !survey_gaps.is_empty() {
                eprintln!();
                eprint!("{}", roundhouse::ingest::survey::render_report(&survey_gaps));
            } else if !survey {
                eprintln!(
                    "roundhouse: hint: re-run with --survey to record unsupported \
                     constructs as a punch list and keep going"
                );
            }
            return Err(format!("ingest {}: {e}", input.display()));
        }
    };
    // Analyze + the post-analyze shared lowerings — type-directed IR
    // rewrites every target consumes (blank-predicate grounding,
    // `Time.current`). The returned residue diagnostics (sites a pass
    // had to leave dynamic) join the analyze warnings below: same
    // collapse-to-count print policy, same --allow-unsupported
    // full-inventory behavior.
    //
    // Exception: the Roda conversion target is source-to-source from
    // the INGEST-shape IR — lowering would rewrite the controller
    // bodies into runtime vocabulary (SQL-folded queries, Views::
    // calls), the wrong altitude to re-idiomize into Sequel/Roda from.
    // See `emit::roda`.
    let lower_diags = if target == BuildTarget::Roda {
        Vec::new()
    } else {
        roundhouse::session::analyze_and_lower(&mut app)
    };

    // Analyze-time diagnostics — the same type errors roundhouse-check
    // reports (dispatch failures, unresolved ivars, incompatible ops).
    // Emitters only stub *annotated* sites (body-typer `expr.diagnostic`);
    // walker-found errors would otherwise pass through into target code
    // that fails later in tsc/cargo/runtime with a worse message, so
    // they print and gate here alongside the emit-gap inventory.
    // (Same Roda exception: analyze never ran, so its diagnostics
    // would be all noise.)
    let mut analyze_diags =
        if target == BuildTarget::Roda { Vec::new() } else { diagnose(&app) };
    analyze_diags.extend(lower_diags);

    // A diagnostic whose root cause is a recorded ingest gap is OUR
    // coverage problem, not the app's: the nil placeholder survey mode
    // substituted is what the analyzer then failed to resolve. Downgrade
    // those to notes with the cause attached, so the error count means
    // "findings" rather than "shadows of the gaps listed below".
    roundhouse::analyze::attribution::attribute_ingest_gaps(
        &mut analyze_diags,
        &app,
        &survey_gaps,
    );

    // Emit inside a diagnostic scope so unsupported-construct gaps in
    // any lowerer/emitter are collected rather than lost (issue #28).
    // Each gap still degrades to a stub in the emitted output, so a
    // single run surfaces the whole inventory instead of dying on the
    // first one.
    let (files_result, mut diags) =
        roundhouse::emit::diagnostics::scope(|| project::target_files(&app, input, target));

    // Prism syntax errors recorded during ingest join the emit-gap
    // inventory: same `Diagnostic` shape, same print/policy/gate
    // treatment. They lead the list — earliest phase, and usually the
    // root cause of any downstream emit noise on the recovered AST. The
    // materialized `files` are deferred to after the gate (below) so a
    // gate failure reports every diagnostic before the run aborts.
    diags.splice(0..0, parse_diags);

    // Policy: errors (unsupported constructs and type errors alike) fail
    // the transpile cleanly by default. With --allow-unsupported they
    // downgrade to warnings and the output is written anyway, so the
    // user can inspect the full inventory in one pass.
    if allow_unsupported {
        for d in diags.iter_mut().chain(analyze_diags.iter_mut()) {
            if d.severity == Severity::Error {
                d.severity = Severity::Warning;
            }
        }
    }
    // The emit sink is always small — print it all. Analyze warnings
    // (gradual_untyped above all) can run to hundreds on a large app,
    // so by default they collapse to a count; --allow-unsupported asks
    // for the full inventory and gets every line.
    for d in &diags {
        eprintln!("roundhouse: {}", d.render(&app.sources));
    }
    let mut suppressed_warnings = 0usize;
    for d in &analyze_diags {
        if d.severity == Severity::Error || allow_unsupported {
            eprintln!("roundhouse: {}", d.render(&app.sources));
        } else {
            suppressed_warnings += 1;
        }
    }
    if suppressed_warnings > 0 {
        eprintln!(
            "roundhouse: {suppressed_warnings} analyze warning(s) not shown — \
             rerun with --allow-unsupported to list them"
        );
    }

    // The punch list, before the gate: on an app that fails the gate
    // this is the most useful thing in the run, and printing it after
    // the early `return Err` would mean never printing it at all.
    if !survey_gaps.is_empty() {
        let notes = analyze_diags.iter().filter(|d| d.severity == Severity::Info).count();
        eprintln!();
        eprint!("{}", roundhouse::ingest::survey::render_report(&survey_gaps));
        eprintln!(
            "roundhouse: {} ingest gap(s) recorded; {notes} diagnostic(s) attributed to them",
            survey_gaps.len()
        );
    }

    let errors = diags.iter().filter(|d| d.severity == Severity::Error).count();
    let type_errors = analyze_diags.iter().filter(|d| d.severity == Severity::Error).count();
    if errors + type_errors > 0 {
        return Err(format!(
            "{errors} unsupported/syntax error(s), {type_errors} type error(s) — rerun \
             with --allow-unsupported to write the output anyway"
        ));
    }

    // Gate passed (or every error was downgraded): materialize the emit
    // output, propagating any hard emit failure now.
    let files = files_result?;
    project::write_to_dir(&files, out)?;
    // The app's own binary files — images, fonts, binary test fixtures.
    // They never became `EmittedFile`s (that type's content is a
    // `String`), so before this they were dropped without a word.
    let assets = project::write_binary_assets(&app.binary_assets, &files, out)?;
    eprintln!(
        "roundhouse: wrote {} files to {} ({})",
        files.len() + assets,
        out.display(),
        target.as_str()
    );
    Ok(())
}
