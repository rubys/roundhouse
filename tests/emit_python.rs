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
    // Pass-2 emits one controller module per resource under
    // `app/controllers/`, not a single `controllers.py` file.
    assert!(
        paths.contains(&"app/controllers/posts_controller.py".to_string()),
        "got {paths:?}",
    );
    assert!(paths.contains(&"app/routes.py".to_string()), "got {paths:?}");
    assert!(paths.contains(&"app/route_helpers.py".to_string()), "got {paths:?}");
    assert!(paths.contains(&"app/test_support.py".to_string()), "got {paths:?}");
    assert!(paths.contains(&"app/views.py".to_string()), "got {paths:?}");
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
fn controller_actions_are_module_functions() {
    // Pass-2 shape: one module per controller under
    // `app/controllers/`, with module-level `def <action>(context)`
    // functions returning `http.ActionResponse`. The Router resolves
    // actions via `getattr(module, action_name)`.
    let app = analyzed_app();
    let files = python::emit(&app);
    let content = find(&files, "app/controllers/posts_controller.py");
    for action in &[
        "def index(",
        "def show(",
        "def create(",
        "def destroy(",
    ] {
        assert!(content.contains(action), "missing {action} in:\n{content}");
    }
    assert!(content.contains("http.ActionResponse"), "got:\n{content}");
}

#[test]
fn routes_register_on_router() {
    // Pass-2 shape: routes.py side-effect-imports controller
    // modules and calls `Router.resources(...)` / `Router.root(...)`
    // at module load, wiring the runtime match table.
    let app = analyzed_app();
    let files = python::emit(&app);
    let content = find(&files, "app/routes.py");
    assert!(content.contains("from app.http import Router"), "got:\n{content}");
    assert!(content.contains("Router.reset()"), "got:\n{content}");
    assert!(
        content.contains("from app.controllers import posts_controller as PostsController"),
        "got:\n{content}",
    );
    // tiny-blog uses explicit get/post routes, not `resources`.
    assert!(
        content.contains("Router.get(\"/posts\", PostsController, \"index\")"),
        "got:\n{content}",
    );
}

#[test]
fn route_helpers_emit_path_functions() {
    let app = analyzed_app();
    let files = python::emit(&app);
    let content = find(&files, "app/route_helpers.py");
    assert!(content.contains("def posts_path() -> str"), "got:\n{content}");
    assert!(content.contains("def post_path(id: int"), "got:\n{content}");
}
