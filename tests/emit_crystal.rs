//! Crystal emitter smoke test.

use std::path::{Path, PathBuf};

use roundhouse::analyze::Analyzer;
use roundhouse::emit::crystal;
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
    let files = crystal::emit(&app);
    let paths: Vec<_> = files.iter().map(|f| f.path.display().to_string()).collect();
    assert!(paths.contains(&"src/models.cr".to_string()), "got {paths:?}");
    assert!(paths.contains(&"src/controllers.cr".to_string()), "got {paths:?}");
    assert!(paths.contains(&"src/routes.cr".to_string()), "got {paths:?}");
}

#[test]
fn models_have_typed_properties() {
    let app = analyzed_app();
    let files = crystal::emit(&app);
    let content = find(&files, "src/models.cr");
    assert!(content.contains("class Post"), "got:\n{content}");
    // Crystal's property DSL with type annotation. BigInt-backed IDs
    // map to Int64 by default.
    assert!(content.contains("property id : Int64"), "got:\n{content}");
    assert!(content.contains("property title : String"), "got:\n{content}");
}

#[test]
fn methods_have_return_type_annotations() {
    let app = analyzed_app();
    let files = crystal::emit(&app);
    let content = find(&files, "src/models.cr");
    // `normalize_title` — body is `title.strip`, typed as String.
    assert!(content.contains("def normalize_title : String"), "got:\n{content}");
}

#[test]
fn controllers_emit_as_classes() {
    let app = analyzed_app();
    let files = crystal::emit(&app);
    let content = find(&files, "src/controllers.cr");
    assert!(content.contains("class PostsController"), "got:\n{content}");
    // Phase 4c: every action returns the stub `Response` value.
    assert!(
        content.contains("def create : Roundhouse::Http::Response"),
        "got:\n{content}"
    );
    assert!(
        content.contains("def destroy : Roundhouse::Http::Response"),
        "got:\n{content}"
    );
}

#[test]
fn routes_table_uses_namedtuple_syntax() {
    let app = analyzed_app();
    let files = crystal::emit(&app);
    let content = find(&files, "src/routes.cr");
    assert!(content.contains("ROUTES = ["), "got:\n{content}");
    // NamedTuple-style `{key: value}` entries.
    assert!(
        content.contains("method: :get, path: \"/posts\""),
        "got:\n{content}"
    );
}

#[test]
fn symbols_emit_as_crystal_symbols() {
    let app = analyzed_app();
    let files = crystal::emit(&app);
    let content = find(&files, "src/controllers.cr");
    // Phase 4c: `params[:id]` in the show action lowers through the
    // stub `Params#[]` — both the receiver and the symbol index
    // preserve their native Crystal shapes.
    assert!(content.contains("params[:id]"), "got:\n{content}");
}
