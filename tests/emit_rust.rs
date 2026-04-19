//! Rust emitter smoke test.
//!
//! Scope for now: emit model structs. Expand as the emitter grows.

use std::path::{Path, PathBuf};

use roundhouse::analyze::Analyzer;
use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app;

fn fixture_path() -> &'static Path {
    Path::new("fixtures/tiny-blog")
}

fn analyzed_app() -> roundhouse::App {
    let mut app = ingest_app(fixture_path()).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
fn emits_a_models_file() {
    let app = ingest_app(fixture_path()).expect("ingest");
    let files = rust::emit(&app);
    let paths: Vec<_> = files.iter().map(|f| f.path.display().to_string()).collect();
    assert!(paths.contains(&"src/models.rs".to_string()), "got {paths:?}");
}

#[test]
fn runtime_is_emitted_alongside_models() {
    // The generated `src/models.rs` references `crate::runtime::
    // ValidationError`; the runtime file has to ship with it so the
    // project compiles. It's copied verbatim from the hand-written
    // source under `runtime/rust/`.
    let app = ingest_app(fixture_path()).expect("ingest");
    let files = rust::emit(&app);
    let runtime = files
        .iter()
        .find(|f| f.path == PathBuf::from("src/runtime.rs"))
        .expect("runtime file should be emitted when any model is emitted");
    assert!(
        runtime.content.contains("pub struct ValidationError"),
        "got:\n{}",
        runtime.content
    );
    assert!(
        runtime
            .content
            .contains("pub fn new(field: &str, message: &str) -> Self"),
        "got:\n{}",
        runtime.content
    );
    assert!(
        runtime.content.contains("pub fn full_message(&self) -> String"),
        "got:\n{}",
        runtime.content
    );
}

#[test]
fn runtime_compiles_as_rust() {
    // The runtime source is `include_str!`d at compile time from
    // `runtime/rust/runtime.rs`. If that file ever stops being valid
    // Rust, the build of *this crate* fails, which is a strong enough
    // check for most changes. The inline `#[cfg(test)]` tests inside
    // runtime.rs itself aren't run by our test harness (they're not
    // compiled into *our* crate — they're part of the string), so
    // this stub just pins the file's presence and non-emptiness.
    let app = ingest_app(fixture_path()).expect("ingest");
    let files = rust::emit(&app);
    let runtime = files
        .iter()
        .find(|f| f.path == PathBuf::from("src/runtime.rs"))
        .unwrap();
    assert!(!runtime.content.is_empty());
    // The include_str! preserves the file verbatim including the
    // `#[cfg(test)] mod tests` block, which means a generated project
    // running `cargo test` also exercises the runtime's own tests.
    assert!(runtime.content.contains("#[cfg(test)]"));
}

#[test]
fn post_struct_is_emitted() {
    let app = ingest_app(fixture_path()).expect("ingest");
    let files = rust::emit(&app);
    let models = files
        .iter()
        .find(|f| f.path == PathBuf::from("src/models.rs"))
        .unwrap();
    assert!(models.content.contains("pub struct Post {"), "got:\n{}", models.content);
    assert!(models.content.contains("pub id: i64,"), "got:\n{}", models.content);
    assert!(models.content.contains("pub title: String,"), "got:\n{}", models.content);
}

#[test]
fn comment_struct_is_emitted_with_foreign_key() {
    let app = ingest_app(fixture_path()).expect("ingest");
    let files = rust::emit(&app);
    let models = files
        .iter()
        .find(|f| f.path == PathBuf::from("src/models.rs"))
        .unwrap();
    assert!(models.content.contains("pub struct Comment {"));
    assert!(models.content.contains("pub body: String,"));
    assert!(models.content.contains("pub post_id: i64,"));
}

#[test]
fn emitted_rust_output_is_stable() {
    // Needs the analyzer — model methods have inferred return types.
    let app = analyzed_app();
    let files = rust::emit(&app);
    let models = files
        .iter()
        .find(|f| f.path == PathBuf::from("src/models.rs"))
        .unwrap();
    // Full snapshot is large — spot-check the shape instead. Every
    // model gets: struct + impl with save/destroy/count/find/all/
    // last/reload + (if validated) validate. Exact per-line text is
    // covered by `validations_emit_as_inline_evaluator` and the
    // persistence tests.
    assert!(
        models.content.contains("pub struct Comment {"),
        "Comment struct missing:\n{}",
        models.content
    );
    assert!(
        models.content.contains("pub struct Post {"),
        "Post struct missing:\n{}",
        models.content
    );
    // Both models now get an impl with the full persistence surface
    // (previously only models with methods/validations got one).
    for method in [
        "impl Comment {",
        "impl Post {",
        "pub fn save(&mut self) -> bool {",
        "pub fn destroy(&self) {",
        "pub fn count() -> i64 {",
        "pub fn find(id: i64) -> Option<",
        "pub fn all() -> Vec<",
        "pub fn last() -> Option<",
        "pub fn reload(&mut self) {",
    ] {
        assert!(
            models.content.contains(method),
            "missing {method:?} in models.rs:\n{}",
            models.content,
        );
    }
}

#[test]
fn validations_emit_as_inline_evaluator() {
    // Build an ad-hoc model with multiple rules to exercise the full
    // Check-to-Rust rendering. tiny-blog's Post only covers Presence;
    // this pins down MinLength and the error-push boilerplate.
    use roundhouse::{
        ClassId, Model, ModelBodyItem, Row, Symbol, TableRef, Validation, ValidationRule,
    };
    use indexmap::IndexMap;
    let mut attrs = IndexMap::new();
    attrs.insert(Symbol::from("id"), roundhouse::Ty::Int);
    attrs.insert(Symbol::from("title"), roundhouse::Ty::Str);
    attrs.insert(Symbol::from("body"), roundhouse::Ty::Str);
    let mut app = roundhouse::App::new();
    app.models.push(Model {
        name: ClassId(Symbol::from("Article")),
        parent: None,
        table: TableRef(Symbol::from("articles")),
        attributes: Row { fields: attrs, rest: None },
        body: vec![
            ModelBodyItem::Validation {
                validation: Validation {
                    attribute: Symbol::from("title"),
                    rules: vec![ValidationRule::Presence],
                },
                leading_comments: vec![],
                leading_blank_line: false,
            },
            ModelBodyItem::Validation {
                validation: Validation {
                    attribute: Symbol::from("body"),
                    rules: vec![
                        ValidationRule::Presence,
                        ValidationRule::Length { min: Some(10), max: None },
                    ],
                },
                leading_comments: vec![],
                leading_blank_line: false,
            },
        ],
    });
    let files = rust::emit(&app);
    let models = files
        .iter()
        .find(|f| f.path == PathBuf::from("src/models.rs"))
        .unwrap();
    // Import is added when any model has validations.
    assert!(
        models.content.contains("use crate::runtime;"),
        "got:\n{}",
        models.content
    );
    // validate() signature uses the runtime-provided error type.
    assert!(
        models
            .content
            .contains("pub fn validate(&self) -> Vec<runtime::ValidationError>"),
        "got:\n{}",
        models.content
    );
    // Presence renders as `.is_empty()`, message is the Rails default.
    assert!(
        models.content.contains("if self.title.is_empty() {"),
        "got:\n{}",
        models.content
    );
    assert!(
        models
            .content
            .contains("runtime::ValidationError::new(\"title\", \"can't be blank\")"),
        "got:\n{}",
        models.content
    );
    // Length fans out into a separate `.len() < n` check.
    assert!(
        models.content.contains("if self.body.len() < 10 {"),
        "got:\n{}",
        models.content
    );
    assert!(
        models.content.contains("is too short (minimum is 10 characters)"),
        "got:\n{}",
        models.content
    );
    // Trailing `errors` return value wraps the evaluator.
    assert!(models.content.contains("        errors\n    }"),
        "got:\n{}", models.content);
}

// Controller emission --------------------------------------------------

#[test]
fn controller_file_is_emitted() {
    let app = analyzed_app();
    let files = rust::emit(&app);
    let paths: Vec<_> = files.iter().map(|f| f.path.display().to_string()).collect();
    assert!(
        paths.contains(&"src/controllers/posts_controller.rs".to_string()),
        "got {paths:?}"
    );
}

#[test]
fn index_action_returns_response_and_calls_model_all() {
    // Phase 4d: every action returns axum's `Response` via
    // `impl IntoResponse`; index collects the model's `all()` and
    // hands off to the views module.
    let app = analyzed_app();
    let files = rust::emit(&app);
    let ctrl = files
        .iter()
        .find(|f| f.path == PathBuf::from("src/controllers/posts_controller.rs"))
        .unwrap();
    assert!(
        ctrl.content.contains("pub async fn index() -> Response"),
        "expected async index returning Response; got:\n{}",
        ctrl.content
    );
    assert!(
        ctrl.content.contains("Post::all()"),
        "expected `Post::all()` call; got:\n{}",
        ctrl.content
    );
}

#[test]
fn show_action_takes_path_id_and_calls_model_find() {
    // Phase 4d: `:id` comes from axum's Path extractor rather than
    // a `params[:id]` call through an HTTP stub.
    let app = analyzed_app();
    let files = rust::emit(&app);
    let ctrl = files
        .iter()
        .find(|f| f.path == PathBuf::from("src/controllers/posts_controller.rs"))
        .unwrap();
    assert!(
        ctrl.content.contains("pub async fn show("),
        "expected async show; got:\n{}",
        ctrl.content
    );
    assert!(
        ctrl.content.contains("Path(id): Path<i64>"),
        "expected axum Path<i64> extractor; got:\n{}",
        ctrl.content
    );
    assert!(
        ctrl.content.contains("Post::find(id)"),
        "expected `Post::find(id)` lookup; got:\n{}",
        ctrl.content
    );
}

#[test]
fn controller_output_is_stable() {
    // Phase 4d posts controller. Actions are axum-native: async
    // fns, Path/Form extractors, `impl IntoResponse` returns. The
    // route-helper + view module paths are predictable enough to
    // snapshot-assert literally.
    let app = analyzed_app();
    let files = rust::emit(&app);
    let ctrl = files
        .iter()
        .find(|f| f.path == PathBuf::from("src/controllers/posts_controller.rs"))
        .unwrap();
    let expected = "\
// Generated by Roundhouse.
#![allow(unused_imports, unused_variables, unused_mut)]

use std::collections::HashMap;

use axum::extract::{Form, Path};
use axum::response::{IntoResponse, Response};

use crate::http::{self, Params};
use crate::models::*;
use crate::route_helpers;
use crate::views;

pub async fn index() -> Response {
    let records: Vec<Post> = Post::all();
    http::html(views::posts_index(&records)).into_response()
}

pub async fn show(
    Path(id): Path<i64>,
) -> Response {
    let record = Post::find(id).unwrap_or_default();
    http::html(views::post_show(&record)).into_response()
}

pub async fn create(
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let p = Params::new(form);
    let fields = p.expect(\"post\", &[\"title\"]);
    let mut record = Post {
        title: fields.get(\"title\").cloned().unwrap_or_default(),
        ..Default::default()
    };
    if record.save() {
        http::redirect(&route_helpers::post_path(record.id)).into_response()
    } else {
        http::unprocessable(views::post_new(&record)).into_response()
    }
}

pub async fn destroy(
    Path(id): Path<i64>,
) -> Response {
    if let Some(record) = Post::find(id) { record.destroy(); }
    http::redirect(&route_helpers::posts_path()).into_response()
}
";
    assert_eq!(ctrl.content, expected);
}
