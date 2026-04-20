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
fn controllers_emit_as_modules_of_self_actions() {
    let app = analyzed_app();
    let files = crystal::emit(&app);
    let content = find(&files, "src/controllers.cr");
    // Pass-2: each controller emits as a module `<Name>Actions`
    // holding `def self.<action>(context) : ActionResponse` methods.
    assert!(
        content.contains("module PostsControllerActions"),
        "got:\n{content}"
    );
    assert!(
        content.contains(
            "def self.create(context : Roundhouse::Http::ActionContext) : Roundhouse::Http::ActionResponse"
        ),
        "got:\n{content}"
    );
    assert!(
        content.contains(
            "def self.destroy(context : Roundhouse::Http::ActionContext) : Roundhouse::Http::ActionResponse"
        ),
        "got:\n{content}"
    );
}

#[test]
fn routes_register_handlers_on_router() {
    let app = analyzed_app();
    let files = crystal::emit(&app);
    let content = find(&files, "src/routes.cr");
    // Pass-2: routes file pushes procs into `Roundhouse::Http::Router`
    // at require time. The old ROUTES NamedTuple constant is gone —
    // TestClient dispatches via `Router.match`.
    assert!(
        content.contains("Roundhouse::Http::Router.add"),
        "got:\n{content}"
    );
    assert!(
        content.contains("PostsControllerActions.index(ctx)"),
        "got:\n{content}"
    );
}

#[test]
fn controller_params_coerce_to_int64() {
    let app = analyzed_app();
    let files = crystal::emit(&app);
    let content = find(&files, "src/controllers.cr");
    // Walker path: `params[:id]` renders inline as
    // `context.params["id"].to_i64` at the ModelFind call site.
    // The prior scaffold template introduced an intermediate
    // `id = ...` binding; the walker doesn't, but the runtime
    // behavior matches.
    assert!(
        content.contains("context.params[\"id\"].to_i64"),
        "got:\n{content}"
    );
    assert!(
        content.contains("Post.find(context.params[\"id\"].to_i64)"),
        "got:\n{content}"
    );
}
