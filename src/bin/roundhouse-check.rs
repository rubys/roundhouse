//! `roundhouse-check` — run analyze + diagnose on a Rails app and
//! print the diagnostics. Exit zero if empty, one if not.
//!
//! This is the first user-facing path for roundhouse's typed IR
//! diagnostics. It's the "did my Ruby parse and type cleanly?" check:
//! point it at a fixture or a real Rails app, get back a list of sites
//! the analyzer flagged (unresolved ivars, method dispatch failures,
//! incompatible operator uses) plus any Prism syntax errors collected
//! during ingest. Parse errors and analyze errors both gate the exit
//! code; warnings and survey gaps are informational.
//!
//! Today's output is message-only — spans are not yet resolvable to
//! file:line:column. Identifier names in the messages are the user's
//! grep targets until real span infrastructure lands (tracked
//! separately; see blog post 3416 for the sketch).
//!
//! Usage:
//!
//!     cargo run --bin roundhouse-check -- [--continue] [FIXTURE]
//!
//! Default FIXTURE is `fixtures/real-blog`.
//!
//! `--continue` (or `ROUNDHOUSE_INGEST_SURVEY=1`) activates survey
//! mode: ingest gaps are recorded instead of aborting, and a
//! deduplicated punch list is printed at the end. Useful for
//! scope-estimation passes on unfamiliar fixtures.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use roundhouse::analyze::{diagnose, Analyzer, Severity};
use roundhouse::ingest::{ingest_app, survey, IngestError};

fn main() -> ExitCode {
    let mut continue_on_error = std::env::var("ROUNDHOUSE_INGEST_SURVEY")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    let mut fixture: Option<String> = None;

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
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

    let fixture = fixture.unwrap_or_else(|| "fixtures/real-blog".into());
    let path = Path::new(&fixture);

    if continue_on_error {
        survey::activate();
    }

    // Ingest inside a parse-diagnostic scope so Prism syntax errors —
    // which the error-recovering parser otherwise drops — are collected
    // and reported alongside the analyze diagnostics.
    let (ingest_result, parse_diags) =
        roundhouse::ingest::prism::scope(|| ingest_app(path));

    let survey_errors = if continue_on_error { survey::drain() } else { Vec::new() };

    let mut app = match ingest_result {
        Ok(app) => app,
        Err(err) => {
            eprintln!("roundhouse-check: ingest failed: {err}");
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
    roundhouse::analyze::attribution::attribute_ingest_gaps(&mut diags, &app, &survey_errors);

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
    let mut buckets: BTreeMap<String, Vec<&IngestError>> = BTreeMap::new();
    for err in errors {
        buckets
            .entry(survey::bucket_key(err))
            .or_default()
            .push(err);
    }

    let mut sorted: Vec<(&String, &Vec<&IngestError>)> = buckets.iter().collect();
    sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));

    eprintln!();
    eprintln!(
        "── Survey: {} ingest gap(s), {} distinct kind(s) ──",
        errors.len(),
        sorted.len(),
    );
    for (key, items) in sorted {
        eprintln!("  [{}×] {}", items.len(), key);
        // Show up to 4 file locations per bucket; collapse the tail.
        let mut shown = std::collections::BTreeSet::new();
        for err in items.iter().take(64) {
            if let IngestError::Unsupported { file, .. } | IngestError::Parse { file, .. } = err {
                shown.insert(file.clone());
            }
        }
        let mut files: Vec<_> = shown.into_iter().collect();
        files.sort();
        for f in files.iter().take(4) {
            eprintln!("        {f}");
        }
        if files.len() > 4 {
            eprintln!("        … and {} more file(s)", files.len() - 4);
        }
    }
}
