//! Real-blog ingest forcing functions.
//!
//! The real-blog fixture is a modernized Rails 8 demo. These tests pin
//! ingest correctness (every recognizer the fixture exercises stays
//! green) and analyzer coverage (full type analysis without annotations).
//! Emit-level forcing functions for the spinel-shape pipeline live in
//! `tests/spinel_toolchain.rs` and `tests/lowered_ruby_emit.rs`.

use std::path::Path;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::ingest::ingest_app;

fn fixture_path() -> &'static Path {
    Path::new("fixtures/real-blog")
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
    let warnings: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == roundhouse::analyze::Severity::Warning)
        .collect();
    eprintln!(
        "real-blog: {} error(s), {} warning(s) (GradualUntyped sites)",
        errors.len(),
        warnings.len()
    );
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
