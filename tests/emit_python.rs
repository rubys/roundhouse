//! Python emitter smoke test.

use std::path::{Path, PathBuf};

use roundhouse::analyze::Analyzer;
use roundhouse::emit::python;
use roundhouse::ingest::ingest_app;

fn fixture_path() -> &'static Path {
    Path::new("fixtures/tiny-blog")
}

fn analyzed_app() -> roundhouse::App {
    let mut app = ingest_app(fixture_path()).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn find<'a>(files: &'a [roundhouse::emit::EmittedFile], p: &str) -> &'a str {
    files
        .iter()
        .find(|f| f.path == PathBuf::from(p))
        .map(|f| f.content.as_str())
        .unwrap_or_else(|| panic!("missing file: {p}"))
}

#[test]
fn emits_expected_files() {
    let app = analyzed_app();
    let files = python::emit(&app);
    let paths: Vec<_> = files.iter().map(|f| f.path.display().to_string()).collect();
    assert!(paths.contains(&"app/models.py".to_string()), "got {paths:?}");
    assert!(paths.contains(&"app/controllers.py".to_string()), "got {paths:?}");
    assert!(paths.contains(&"app/routes.py".to_string()), "got {paths:?}");
}

#[test]
fn models_are_classes_with_type_hints() {
    let app = analyzed_app();
    let files = python::emit(&app);
    let content = find(&files, "app/models.py");
    assert!(content.contains("from __future__ import annotations"), "got:\n{content}");
    assert!(content.contains("class Post:"), "got:\n{content}");
    assert!(content.contains("class Comment:"), "got:\n{content}");
    // Field type hints use PEP 585 built-in generics and PEP 604
    // union syntax. tiny-blog's Post has id (int) + title (str).
    assert!(content.contains("id: int"), "got:\n{content}");
    assert!(content.contains("title: str"), "got:\n{content}");
}

#[test]
fn model_methods_annotate_return_type() {
    let app = analyzed_app();
    let files = python::emit(&app);
    let content = find(&files, "app/models.py");
    // `normalize_title` returns `title.strip()` — analyzer types it
    // as str.
    assert!(content.contains("def normalize_title(self) -> str:"), "got:\n{content}");
}

#[test]
fn controller_actions_are_async_methods() {
    let app = analyzed_app();
    let files = python::emit(&app);
    let content = find(&files, "app/controllers.py");
    assert!(content.contains("class PostsController:"), "got:\n{content}");
    for action in &["async def index(self)", "async def show(self)", "async def create(self)", "async def destroy(self)"] {
        assert!(content.contains(action), "missing {action} in:\n{content}");
    }
}

#[test]
fn routes_file_is_a_list_of_dicts() {
    let app = analyzed_app();
    let files = python::emit(&app);
    let content = find(&files, "app/routes.py");
    assert!(content.contains("routes: list[dict] = ["), "got:\n{content}");
    assert!(content.contains("\"method\": \"GET\""), "got:\n{content}");
    assert!(content.contains("\"path\": \"/posts\""), "got:\n{content}");
    assert!(content.contains("\"controller\": \"PostsController\""), "got:\n{content}");
}
