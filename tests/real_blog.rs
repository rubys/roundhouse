//! Real-blog forcing functions.
//!
//! The real-blog fixture is a modernized Rails 8 demo — the target for
//! Phase 1 of the multi-target plan. Once ingest succeeds end-to-end, we
//! pair it with the same two forcing functions as tiny-blog:
//!
//! 1. **source_equivalence** — emitted Ruby equals the fixture source
//!    byte-for-byte. Catches silent drops in ingest (a construct the
//!    recognizer didn't know about, so emit has nothing to emit).
//! 2. **round_trip_identity** — ingest → emit → ingest yields the same
//!    IR. Catches IR holes (emit produced a form the ingester doesn't
//!    recognize, or the IR dropped information the emitter needed).
//!
//! `EXPECTED_RUBY_FILES` is the inclusion list: every file on it must
//! round-trip cleanly under both checks. Files not on the list are still
//! ingested (so holes fail loud) but excluded from byte-equivalence until
//! the remaining recognizers land. As gaps close, promote files onto
//! the list.

use std::path::{Path, PathBuf};

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app;

fn fixture_path() -> &'static Path {
    Path::new("fixtures/real-blog")
}

/// Files expected to round-trip byte-for-byte today. Grows as recognizers
/// catch up to the fixture. Anything NOT listed here is still ingested
/// (so IR-level failures surface), but source-equivalence is skipped.
///
/// Known-excluded (remaining gaps — see fixtures/real-blog/README.md):
/// - `db/migrate/*.rb` — migrations (we read `db/schema.rb` today; Rails 8
///   doesn't generate `schema.rb` until migrations run).
/// - `test/**/*.rb` — not yet ingested as part of app/ pipeline.
/// - `app/models/*.rb` — `broadcasts_to`, `after_*_commit { block }`,
///   extra validation rules, comments still drop.
/// - `app/controllers/*.rb` — `private` marker, comments, unknown
///   class-body calls still drop.
/// - `app/views/**/*.erb` — multi-line argument formatting doesn't
///   round-trip yet.
const EXPECTED_RUBY_FILES: &[&str] = &[
    "app/controllers/application_controller.rb",
    "app/controllers/articles_controller.rb",
    "app/controllers/comments_controller.rb",
    "app/models/application_record.rb",
    "app/models/article.rb",
    "app/models/comment.rb",
    "app/views/articles/_article.html.erb",
    "app/views/articles/edit.html.erb",
    "app/views/articles/index.html.erb",
    "app/views/articles/new.html.erb",
    "config/routes.rb",
];

fn scratch_root(suffix: &str) -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("roundhouse").join(suffix)
}

fn write_emitted(dir: &Path, app: &roundhouse::App) {
    if dir.exists() {
        std::fs::remove_dir_all(dir).expect("clean scratch dir");
    }
    std::fs::create_dir_all(dir).expect("create scratch dir");
    for file in ruby::emit(app) {
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir -p");
        }
        std::fs::write(&path, &file.content).expect("write");
    }
}

#[test]
fn ingests_without_errors() {
    // Loud failure if any recognizer regresses. Ingest is expected to
    // complete today — the known unsupported constructs (nested route
    // DSL, migrations, test files) live in files the app walker doesn't
    // touch yet.
    let app = ingest_app(fixture_path()).expect("ingest real-blog");
    assert!(!app.models.is_empty(), "expected at least one model");
    assert!(!app.controllers.is_empty(), "expected at least one controller");
    assert!(!app.views.is_empty(), "expected at least one view");
}

#[test]
fn expected_files_round_trip_byte_for_byte() {
    if EXPECTED_RUBY_FILES.is_empty() {
        // Still bootstrapping — no files on the inclusion list yet. The
        // `ingests_without_errors` test keeps ingest honest; promote
        // individual files here once they round-trip cleanly.
        return;
    }
    let app = ingest_app(fixture_path()).expect("ingest");
    let emitted = ruby::emit(&app);
    let expected: std::collections::HashSet<PathBuf> = EXPECTED_RUBY_FILES
        .iter()
        .map(PathBuf::from)
        .collect();

    for file in &emitted {
        if !expected.contains(&file.path) {
            continue;
        }
        let fixture_file = fixture_path().join(&file.path);
        let source = std::fs::read_to_string(&fixture_file)
            .unwrap_or_else(|e| panic!("read {}: {e}", fixture_file.display()));
        assert_eq!(
            file.content, source,
            "emitted {} differs from fixture source.\n--- emitted ---\n{}\n--- source ---\n{}",
            file.path.display(),
            file.content,
            source
        );
    }
}

#[test]
fn ir_is_fixed_under_emit_ingest() {
    let original = ingest_app(fixture_path()).expect("ingest original");

    let scratch = scratch_root("real_blog_round_trip");
    write_emitted(&scratch, &original);

    let roundtripped = ingest_app(&scratch).expect("ingest re-emitted");

    assert_eq!(
        original, roundtripped,
        "IR diverged across Ruby emit + re-ingest"
    );
}

// Type-analysis coverage forcing function ---------------------------------
//
// Runs the analyzer + diagnose() over every controller/model/view in
// real-blog and asserts zero diagnostics. real-blog is the baseline
// "basic MVC Rails app" — full analysis without annotations is the
// promise, and this test enforces it.

fn diagnostic_signature(d: &roundhouse::analyze::Diagnostic) -> (String, String) {
    match &d.kind {
        DiagnosticKind::IvarUnresolved { name } => {
            ("IvarUnresolved".into(), format!("@{}", name.as_str()))
        }
        DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
            let recv_descriptor = match recv_ty {
                roundhouse::ty::Ty::Class { id, .. } => format!("Class({})", id.0.as_str()),
                roundhouse::ty::Ty::Array { elem } => match &**elem {
                    roundhouse::ty::Ty::Class { id, .. } => {
                        format!("Array<Class({})>", id.0.as_str())
                    }
                    other => format!("Array<{other:?}>"),
                },
                roundhouse::ty::Ty::Hash { key, value } => {
                    format!("Hash<{:?}, {:?}>", key, value)
                }
                other => format!("{other:?}"),
            };
            ("SendDispatchFailed".into(), format!("{}:{}", method.as_str(), recv_descriptor))
        }
    }
}

#[test]
fn type_analysis_coverage() {
    // real-blog is fully type-analyzable with no annotations. Every
    // expression's ty is concrete, so diagnose() yields an empty list.
    //
    // When this starts failing, the delta is the work queue: either a
    // new dialect gap surfaced (extend the registry in src/analyze.rs)
    // or an existing registry entry stopped firing (the fixture changed
    // shape). Keep this tight — the promise of "full analysis without
    // annotations on a basic MVC blog" is only a promise if we enforce
    // it on every commit.
    let mut app = ingest_app(fixture_path()).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let diags = diagnose(&app);
    assert!(
        diags.is_empty(),
        "real-blog has {} diagnostics (expected zero):\n{}",
        diags.len(),
        diags
            .iter()
            .map(|d| {
                let (kind, detail) = diagnostic_signature(d);
                format!("  {kind}: {detail}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    );
}
