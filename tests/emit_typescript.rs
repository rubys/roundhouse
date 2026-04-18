//! TypeScript emitter smoke test.
//!
//! Phase 2 scaffold — asserts the emitter produces the expected files
//! and their top-level shapes. The output isn't runnable TypeScript
//! yet (no runtime imports, no template emission); once Phase 3 adds
//! Juntos integration these tests grow to cover the runtime surface.

use std::path::{Path, PathBuf};

use roundhouse::analyze::Analyzer;
use roundhouse::emit::typescript;
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
    let files = typescript::emit(&app);
    let paths: Vec<_> = files.iter().map(|f| f.path.display().to_string()).collect();
    assert!(paths.contains(&"src/models.ts".to_string()), "got {paths:?}");
    assert!(paths.contains(&"src/controllers.ts".to_string()), "got {paths:?}");
    assert!(paths.contains(&"src/routes.ts".to_string()), "got {paths:?}");
}

#[test]
fn model_classes_are_typed() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "src/models.ts");
    // Each model becomes a class.
    assert!(content.contains("export class Post {"), "got:\n{content}");
    assert!(content.contains("export class Comment {"), "got:\n{content}");
    // Schema-derived fields get camelCased TS names with type annotations.
    assert!(content.contains("id: number;"), "got:\n{content}");
    assert!(content.contains("title: string;"), "got:\n{content}");
    assert!(content.contains("postId: number;"), "got:\n{content}");
}

#[test]
fn model_methods_emit_with_return_types() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "src/models.ts");
    // `normalize_title` in tiny-blog Post; it has a string body
    // (title.strip), so the analyzer types it as string.
    assert!(
        content.contains("normalizeTitle(): string"),
        "got:\n{content}"
    );
}

#[test]
fn controller_actions_are_async() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "src/controllers.ts");
    assert!(content.contains("export class PostsController {"), "got:\n{content}");
    // Every action is emitted as `async <name>()`. Tiny-blog's
    // PostsController has index / show / create / destroy.
    for action in &["async index()", "async show()", "async create()", "async destroy()"] {
        assert!(content.contains(action), "missing {action} in:\n{content}");
    }
    // Async methods wrap their return type in Promise<…>.
    assert!(content.contains("Promise<void>"), "missing Promise<void> in:\n{content}");
    assert!(content.contains("Promise<Post"), "missing Promise<Post…> in:\n{content}");
}

#[test]
fn routes_dispatch_table_has_every_route() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "src/routes.ts");
    assert!(content.contains("export interface Route {"), "got:\n{content}");
    assert!(content.contains("export const routes: Route[] = ["), "got:\n{content}");
    // tiny-blog declares four explicit verb routes; each shows up as a row.
    assert!(content.contains("method: \"GET\""), "got:\n{content}");
    assert!(content.contains("path: \"/posts\""), "got:\n{content}");
    assert!(content.contains("controller: \"PostsController\""), "got:\n{content}");
    assert!(content.contains("action: \"index\""), "got:\n{content}");
}
