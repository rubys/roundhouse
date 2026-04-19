//! Elixir emitter smoke test.
//!
//! Phase 2 scaffold — asserts the emitter produces the expected files
//! and their top-level shapes. Elixir is the target that most
//! aggressively stress-tests IR neutrality (no classes, no mutation,
//! no method dispatch), so these tests also double as evidence that
//! the typed IR isn't secretly class-shaped.

use std::path::{Path, PathBuf};

use roundhouse::analyze::Analyzer;
use roundhouse::emit::elixir;
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
    let files = elixir::emit(&app);
    let paths: Vec<_> = files.iter().map(|f| f.path.display().to_string()).collect();
    // One .ex per model and per controller, plus the router.
    assert!(paths.contains(&"lib/post.ex".to_string()), "got {paths:?}");
    assert!(paths.contains(&"lib/comment.ex".to_string()), "got {paths:?}");
    assert!(paths.contains(&"lib/posts_controller.ex".to_string()), "got {paths:?}");
    assert!(paths.contains(&"lib/router.ex".to_string()), "got {paths:?}");
}

#[test]
fn models_define_struct_and_module_functions() {
    let app = analyzed_app();
    let files = elixir::emit(&app);
    let content = find(&files, "lib/post.ex");
    assert!(content.contains("defmodule Post do"), "got:\n{content}");
    // Phase 3: defstruct declares typed defaults so NOT NULL schema
    // columns get concrete values before save runs (SQLite rejects
    // nil → NOT NULL).
    assert!(
        content.contains("defstruct [id: nil, title: \"\"]"),
        "got:\n{content}"
    );
    // Instance methods become module functions taking the record as
    // first arg. `normalize_title` from tiny-blog's Post.
    assert!(
        content.contains("def normalize_title(post) do"),
        "got:\n{content}"
    );
}

#[test]
fn ivar_reads_become_struct_field_access_in_instance_methods() {
    let app = analyzed_app();
    let files = elixir::emit(&app);
    let content = find(&files, "lib/post.ex");
    // Ruby body `title.strip` — `title` is a bareword call, emitted
    // as-is (not an ivar in this case). This test is really a
    // regression guard for the method signature shape; extend it
    // when a fixture lands that reads `@foo` inside an instance method.
    assert!(content.contains("def normalize_title(post) do"), "got:\n{content}");
}

#[test]
fn controllers_emit_as_bare_modules() {
    let app = analyzed_app();
    let files = elixir::emit(&app);
    let content = find(&files, "lib/posts_controller.ex");
    assert!(content.contains("defmodule PostsController do"), "got:\n{content}");
    // Phase 4c: each controller imports the stub HTTP surface (so
    // bodies can call `render`, `redirect_to`, etc. bare) and every
    // action is `def <name>(...)` — arg name is `params` or `_params`
    // depending on whether the emitted body references it.
    assert!(
        content.contains("import Roundhouse.Http"),
        "expected Roundhouse.Http import; got:\n{content}"
    );
    for action in &[
        "def index(",
        "def show(",
        "def create(",
        "def destroy(",
    ] {
        assert!(content.contains(action), "missing {action} in:\n{content}");
    }
}

#[test]
fn ivar_writes_become_local_rebinds_in_controller_actions() {
    let app = analyzed_app();
    let files = elixir::emit(&app);
    let content = find(&files, "lib/posts_controller.ex");
    // Ruby `@post = Post.find(params[:id])` in `show` becomes a
    // straightforward Elixir rebind — the `@` dies.
    assert!(
        content.contains("post = Post.find(params[:id])"),
        "got:\n{content}"
    );
}

#[test]
fn router_is_a_module_with_routes_list() {
    let app = analyzed_app();
    let files = elixir::emit(&app);
    let content = find(&files, "lib/router.ex");
    assert!(content.contains("defmodule Router do"), "got:\n{content}");
    assert!(content.contains("@routes ["), "got:\n{content}");
    // Each route as a keyword map with atom method / controller /
    // action. Path stays a string.
    assert!(
        content.contains("method: :get, path: \"/posts\""),
        "got:\n{content}"
    );
    assert!(
        content.contains("controller: PostsController, action: :index"),
        "got:\n{content}"
    );
}

#[test]
fn symbols_emit_as_atoms() {
    let app = analyzed_app();
    let files = elixir::emit(&app);
    let content = find(&files, "lib/posts_controller.ex");
    // `params[:id]` — `:id` is a Ruby symbol, which maps 1:1 to an
    // Elixir atom. Bracket-access syntax carries through.
    assert!(content.contains("params[:id]"), "got:\n{content}");
}
