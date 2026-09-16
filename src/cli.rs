//! The subcommands behind the multi-call `roundhouse` binary and the
//! per-tool aliases (`roundhouse-check`, `roundhouse-lsp`,
//! `roundhouse-mcp`). Both spellings run the code here, so a shipped
//! single binary and a `cargo run --bin` dev loop cannot drift.
//!
//! Host-only: every subcommand speaks stdio or walks a real directory.

use std::path::Path;
use std::process::ExitCode;

use crate::analyze::{diagnose, Analyzer, Severity};
use crate::ingest::{ingest_app, survey, IngestError};

/// `roundhouse lsp`: serve the read-only Language Server over stdio
/// until the client hangs up. The workspace root comes from the LSP
/// `initialize` request, so there are no arguments.
pub fn lsp(args: &[String]) -> ExitCode {
    if let Some(bad) = args.iter().find(|a| !matches!(a.as_str(), "-h" | "--help")) {
        eprintln!("roundhouse lsp: unexpected argument {bad}");
        return ExitCode::from(2);
    }
    if !args.is_empty() {
        println!("Usage: roundhouse lsp

Serve the Language Server over stdio; the editor names the workspace.");
        return ExitCode::SUCCESS;
    }
    match crate::lsp::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("roundhouse-lsp: fatal: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `roundhouse mcp [APP]`: serve the MCP tools over stdio for the Rails
/// app at `APP` (else `$ROUNDHOUSE_APP_ROOT`, else the working
/// directory) until the client hangs up.
pub fn mcp(args: &[String]) -> ExitCode {
    let mut root: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "Usage: roundhouse mcp [APP]

Serve the MCP tools over stdio for the Rails app at APP
(default: $ROUNDHOUSE_APP_ROOT, else the working directory)."
                );
                return ExitCode::SUCCESS;
            }
            s if s.starts_with('-') => {
                eprintln!("roundhouse mcp: unknown flag {s}");
                return ExitCode::from(2);
            }
            _ if root.is_some() => {
                eprintln!("roundhouse mcp: expected at most one APP argument");
                return ExitCode::from(2);
            }
            _ => root = Some(arg.clone()),
        }
    }
    let root = crate::mcp::workspace_root(root);
    // Same guard as `check`: a misconfigured client would otherwise get a
    // server that answers every question about an empty app.
    if !root.join("app").is_dir() {
        eprintln!(
            "roundhouse mcp: {} does not look like a Rails app (no app/ directory)",
            root.display()
        );
        return ExitCode::from(2);
    }
    match crate::mcp::run_at(root) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("roundhouse-mcp: fatal: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `roundhouse check [--continue|--strict] [APP]`: ingest + analyze +
/// diagnose the app at `APP` (default `default_app`) and print the
/// diagnostics. Exit zero if there are no errors, one if there are.
///
/// Point it at a fixture or a real Rails app, get back the sites the
/// analyzer flagged (unresolved ivars, method dispatch failures,
/// incompatible operator uses) plus any Prism syntax errors collected
/// during ingest. Parse errors and analyze errors both gate the exit
/// code; warnings and survey gaps are informational.
///
/// `--continue` (or `ROUNDHOUSE_INGEST_SURVEY=1`) activates survey
/// mode: ingest gaps are recorded instead of aborting, and a
/// deduplicated punch list is printed at the end. Useful for
/// scope-estimation passes on unfamiliar apps.
pub fn check(args: &[String], default_app: &str) -> ExitCode {
    let mut continue_on_error = std::env::var("ROUNDHOUSE_INGEST_SURVEY")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    let mut fixture: Option<String> = None;

    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "Usage: roundhouse check [--continue|--strict] [APP]\n\n\
                     Analyze the Rails app at APP (default {default_app}) and print the\n\
                     diagnostics; exit 1 on parse or type errors.\n\n\
                     Options:\n\
                       --continue   Record unsupported constructs as a punch list and keep\n\
                                    going (also ROUNDHOUSE_INGEST_SURVEY=1).\n\
                       --strict     Abort at the first unsupported construct (default)."
                );
                return ExitCode::SUCCESS;
            }
            "--continue" => continue_on_error = true,
            "--strict" => continue_on_error = false,
            other if other.starts_with("--") => {
                eprintln!("roundhouse-check: unknown flag {other}");
                return ExitCode::from(2);
            }
            other => {
                if fixture.is_some() {
                    eprintln!("roundhouse-check: positional argument given twice");
                    return ExitCode::from(2);
                }
                fixture = Some(other.to_string());
            }
        }
    }

    let fixture = fixture.unwrap_or_else(|| default_app.to_string());
    let path = Path::new(&fixture);

    // A path that is not a Rails app must not check clean: ingest walks
    // whatever is there, and a typo'd directory or a repo root one level
    // above the app would otherwise report zero of everything and exit 0.
    if !path.is_dir() {
        eprintln!("roundhouse-check: {fixture} is not a directory");
        return ExitCode::from(2);
    }
    if !path.join("app").is_dir() {
        eprintln!(
            "roundhouse-check: {fixture} does not look like a Rails app (no app/ directory)"
        );
        return ExitCode::from(2);
    }

    if continue_on_error {
        survey::activate();
    }

    // Ingest inside a parse-diagnostic scope so Prism syntax errors —
    // which the error-recovering parser otherwise drops — are collected
    // and reported alongside the analyze diagnostics.
    let (ingest_result, parse_diags) =
        crate::ingest::prism::scope(|| ingest_app(path));

    let survey_errors = if continue_on_error { survey::drain() } else { Vec::new() };

    let mut app = match ingest_result {
        Ok(app) => app,
        Err(err) => {
            eprintln!("roundhouse-check: ingest failed: {err}");
            if !continue_on_error {
                // First contact with a real app almost always trips a
                // construct the ingester doesn't cover yet; strict mode
                // (the fixture-oriented default) aborts on the first one.
                // Point the user at survey mode instead of dead-ending.
                eprintln!(
                    "roundhouse-check: hint: re-run with --continue to record \
                     unsupported constructs as a punch list and keep going"
                );
            }
            // Surface any syntax errors first — a malformed file is
            // usually the root cause of the construct ingest then choked
            // on. Sources aren't populated on this path, so message-only.
            for d in &parse_diags {
                eprintln!("{}", d.render(&[]));
            }
            // Even on hard failure, surface any partial survey results
            // so the user still gets some signal.
            if !survey_errors.is_empty() {
                print_survey_report(&survey_errors);
            }
            return ExitCode::from(2);
        }
    };
    Analyzer::new(&app).analyze(&mut app);
    let mut diags = diagnose(&app);
    // Survey mode: diagnostics that trace back to a recorded ingest gap
    // are the tool's coverage problem, not the app's — downgrade them to
    // notes with the root cause attached so the error count below means
    // "findings", not "shadows of the gaps listed at the end".
    crate::analyze::attribution::attribute_ingest_gaps(&mut diags, &app, &survey_errors);
    // Likewise a dispatch on a gem the analyzer does not model — the
    // census below names the gems, this labels the diagnostics.
    crate::analyze::attribution::attribute_unknown_gems(&mut diags, &app);
    let census = app.gem_lock.as_ref().map(crate::gems::GemCensus::of);

    let errors = diags.iter().filter(|d| d.severity == Severity::Error).count();
    let warnings = diags.iter().filter(|d| d.severity == Severity::Warning).count();
    let notes = diags.iter().filter(|d| d.severity == Severity::Info).count();
    let parse_errors = parse_diags.iter().filter(|d| d.severity == Severity::Error).count();

    let mut had_output = false;
    // Parse diagnostics lead — earliest phase, and usually the root
    // cause of any downstream analyze noise on the recovered AST.
    for d in &parse_diags {
        eprintln!("{}", d.render(&app.sources));
        had_output = true;
    }
    for d in &diags {
        eprintln!("{}", d.render(&app.sources));
        had_output = true;
    }

    if !survey_errors.is_empty() {
        print_survey_report(&survey_errors);
        had_output = true;
    }

    if had_output {
        eprintln!();
    }
    // The gem census: which of the app's declared gems the analyzer
    // models, which never enter the analysis, and which it does not
    // know — read beside the errors, the unknown list is the
    // prioritization signal.
    if let Some(census) = &census {
        eprintln!("roundhouse-check: {}", census.summary());
    }
    eprintln!(
        "roundhouse-check: {} — {} parse error(s), {} error(s), {} warning(s), {} gap-attributed note(s), {} survey gap(s)",
        fixture,
        parse_errors,
        errors,
        warnings,
        notes,
        survey_errors.len(),
    );

    // Survey errors are informational; they don't gate exit code.
    // Strict-mode ingest errors are caught above. Parse (syntax) errors
    // and analyze errors both gate.
    if errors + parse_errors > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Print a deduplicated, frequency-sorted view of survey-collected
/// ingest gaps. Buckets share a key derived from the message prefix
/// (`survey::bucket_key`) so "ConstantWriteNode at foo.rb" and "...
/// at bar.rb" group into one entry with a file list.
fn print_survey_report(errors: &[IngestError]) {
    eprintln!();
    eprint!("{}", survey::render_report(errors));
}
