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
fn model_tests_ingest_into_test_modules() {
    // Phase 2a forcing function: real-blog has two model test files
    // (article_test.rb, comment_test.rb). Ingest should produce one
    // TestModule per file, each carrying its `test "..." do ... end`
    // declarations as named Test entries with populated bodies.
    let app = ingest_app(fixture_path()).expect("ingest");

    let names: Vec<&str> = app
        .test_modules
        .iter()
        .map(|tm| tm.name.0.as_str())
        .collect();
    assert!(
        names.contains(&"ArticleTest") && names.contains(&"CommentTest"),
        "expected ArticleTest and CommentTest; got {:?}",
        names
    );

    let article_tests = app
        .test_modules
        .iter()
        .find(|tm| tm.name.0.as_str() == "ArticleTest")
        .expect("ArticleTest module");

    // Target class inferred by stripping "Test" suffix.
    assert_eq!(
        article_tests.target.as_ref().map(|c| c.0.as_str()),
        Some("Article"),
    );

    // All 4 article tests should be captured by name.
    let test_names: Vec<&str> =
        article_tests.tests.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        test_names,
        vec![
            "creates an article with valid attributes",
            "validates title presence",
            "validates body minimum length",
            "destroys comments when article is destroyed",
        ],
        "article test names"
    );

    // Each test's body should be non-empty (ingested as an Expr, not a
    // placeholder). The first test's body should contain at least one
    // Send — the `articles(:one)` call.
    let first = &article_tests.tests[0];
    use roundhouse::expr::ExprNode;
    match &*first.body.node {
        ExprNode::Seq { exprs } => {
            assert!(!exprs.is_empty(), "first test body should have statements");
        }
        _ => panic!("first test body should be a Seq, got {:?}", first.body.node),
    }
}

#[test]
fn fixtures_ingest_into_app() {
    // real-blog has two YAML fixture files under test/fixtures/.
    // Each should land in app.fixtures with records preserving label
    // order and field values as strings.
    let app = ingest_app(fixture_path()).expect("ingest");

    let names: Vec<&str> = app.fixtures.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"articles") && names.contains(&"comments"),
        "expected articles and comments fixtures; got {:?}",
        names
    );

    let articles = app
        .fixtures
        .iter()
        .find(|f| f.name.as_str() == "articles")
        .expect("articles fixture");
    let one = articles
        .records
        .get(&roundhouse::Symbol::from("one"))
        .expect("articles: one");
    assert_eq!(
        one.get(&roundhouse::Symbol::from("title")).map(|s| s.as_str()),
        Some("Getting Started with Rails"),
    );
    // Fixture-reference shorthand stays as a string in the IR —
    // resolution is an emit-time concern.
    let comments = app
        .fixtures
        .iter()
        .find(|f| f.name.as_str() == "comments")
        .expect("comments fixture");
    let one_c = comments
        .records
        .get(&roundhouse::Symbol::from("one"))
        .expect("comments: one");
    assert_eq!(
        one_c.get(&roundhouse::Symbol::from("article")).map(|s| s.as_str()),
        Some("one"),
    );
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
        DiagnosticKind::IncompatibleBinop { op, lhs_ty, rhs_ty } => (
            "IncompatibleBinop".into(),
            format!("{lhs_ty:?} {} {rhs_ty:?}", op.as_str()),
        ),
        DiagnosticKind::GradualUntyped { expr_kind } => (
            "GradualUntyped".into(),
            expr_kind.as_str().to_string(),
        ),
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
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == roundhouse::analyze::Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "real-blog has {} error diagnostic(s) (expected zero):\n{}",
        errors.len(),
        errors
            .iter()
            .map(|d| {
                let (kind, detail) = diagnostic_signature(d);
                format!("  {kind}: {detail}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    );
}
