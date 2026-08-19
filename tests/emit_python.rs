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
    // Controllers, dispatch, and the route table live in the overlay
    // (`app/v2/`) — the per-artifact `app/controllers/` modules and the
    // module-handler `app/routes.py` retired with the CtrlWalker
    // teardown.
    assert!(paths.contains(&"app/v2/controllers.py".to_string()), "got {paths:?}");
    assert!(paths.contains(&"app/v2/dispatch.py".to_string()), "got {paths:?}");
    assert!(paths.contains(&"app/v2/routes.py".to_string()), "got {paths:?}");
    assert!(!paths.contains(&"app/routes.py".to_string()), "legacy routes.py returned: {paths:?}");
    assert!(
        !paths.iter().any(|p| p.starts_with("app/controllers/")),
        "legacy per-artifact controllers returned: {paths:?}"
    );
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
    // Models now emit through the shared model->LibraryClass lowering
    // (same path as TS), so each is a thin subclass of ApplicationRecord
    // (-> Base) rather than a self-contained class.
    assert!(content.contains("class Post(ApplicationRecord):"), "got:\n{content}");
    assert!(content.contains("class Comment(ApplicationRecord):"), "got:\n{content}");
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
    // Lowered models annotate return types throughout — e.g. the
    // per-model `table_name` class method the lowering synthesizes.
    // (The old bespoke path's `normalize_title` reader is gone: the
    // shared lowering drops `before_save :symbol` callback methods, same
    // as the TS path.)
    assert!(content.contains("def table_name(cls) -> str:"), "got:\n{content}");
}

#[test]
fn controllers_are_overlay_classes_with_process_action() {
    // Overlay shape: controller CLASSES subclassing the transpiled
    // ActionController Base, with a synthesized `process_action`
    // dispatcher — the universal-IR contract every target shares.
    let app = analyzed_app();
    let files = python::emit(&app);
    let content = find(&files, "app/v2/controllers.py");
    // tiny-blog has no explicit ApplicationController — the overlay
    // aliases the missing parent to the framework Base.
    assert!(content.contains("ApplicationController = Base"), "got:\n{content}");
    assert!(
        content.contains("class PostsController(ApplicationController):"),
        "got:\n{content}"
    );
    assert!(content.contains("def process_action(self, action_name: str)"), "got:\n{content}");
    for action in &["def index(", "def show(", "def create(", "def destroy("] {
        assert!(content.contains(action), "missing {action} in:\n{content}");
    }
}

#[test]
fn routes_emit_as_route_table() {
    // Overlay shape: `app/v2/routes.py` holds the RouteTable data the
    // transpiled `app/router.py` matches over; `app/v2/dispatch.py`'s
    // `handle` runs the full request cycle. No module-handler
    // registration remains.
    let app = analyzed_app();
    let files = python::emit(&app);
    let content = find(&files, "app/v2/routes.py");
    assert!(content.contains("from app.router import Route"), "got:\n{content}");
    assert!(
        content.contains("Route(\"GET\", \"/posts\", \"posts\", \"index\")"),
        "got:\n{content}",
    );
    let dispatch = find(&files, "app/v2/dispatch.py");
    assert!(dispatch.contains("def handle(method: str, path: str"), "got:\n{dispatch}");
    assert!(dispatch.contains("\"posts\": PostsController,"), "got:\n{dispatch}");
}

#[test]
fn route_helpers_emit_path_functions() {
    let app = analyzed_app();
    let files = python::emit(&app);
    let content = find(&files, "app/route_helpers.py");
    assert!(content.contains("def posts_path() -> str"), "got:\n{content}");
    assert!(content.contains("def post_path(id: int"), "got:\n{content}");
}
