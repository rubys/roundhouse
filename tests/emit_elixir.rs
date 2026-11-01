//! Elixir emitter smoke test.
//!
//! Asserts the emitter produces the expected files and their top-level
//! shapes. As of Phase D the output is the elixir (`*`) lowered-IR
//! overlay; these assertions track the v2 module shapes. Elixir is the
//! target that most aggressively stress-tests IR neutrality (no classes,
//! no mutation, no method dispatch), so they also double as evidence that
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
    // The shared project + runtime infra (mix.exs + Roundhouse.Db) the v2
    // stack depends on.
    assert!(paths.contains(&"mix.exs".to_string()), "got {paths:?}");
    assert!(paths.contains(&"lib/roundhouse/db.ex".to_string()), "got {paths:?}");
    // One `*` module per model and per controller, under lib/, plus
    // the routes table + dispatch shim.
    assert!(paths.contains(&"lib/post.ex".to_string()), "got {paths:?}");
    assert!(paths.contains(&"lib/comment.ex".to_string()), "got {paths:?}");
    assert!(paths.contains(&"lib/postscontroller.ex".to_string()), "got {paths:?}");
    assert!(paths.contains(&"lib/routes_table.ex".to_string()), "got {paths:?}");
    assert!(paths.contains(&"lib/dispatch.ex".to_string()), "got {paths:?}");
}

#[test]
fn models_define_struct_and_module_functions() {
    let app = analyzed_app();
    let files = elixir::emit(&app);
    let content = find(&files, "lib/post.ex");
    // Elixir has no classes — a model is a module with a `defstruct`
    // payload and module functions taking the record as first arg.
    assert!(content.contains("defmodule Post do"), "got:\n{content}");
    assert!(content.contains("defstruct ["), "got:\n{content}");
    assert!(content.contains("def table_name() do"), "got:\n{content}");
    // Instance methods become module functions threading the record as the
    // first arg (`update(record, p)`, `get(record, name)`, the synthesized
    // `validate(record)`). NOTE: user-defined custom instance methods + the
    // `before_save :normalize_title` callback aren't emitted by the v2
    // model lowering yet — a known gap, not exercised by real-blog; tracked
    // separately. Assert a method v2 does emit.
    assert!(content.contains("def update(record,"), "got:\n{content}");
    assert!(content.contains("def validate(record) do"), "got:\n{content}");
}

#[test]
fn controllers_emit_as_bare_modules() {
    let app = analyzed_app();
    let files = elixir::emit(&app);
    let content = find(&files, "lib/postscontroller.ex");
    assert!(content.contains("defmodule PostsController do"), "got:\n{content}");
    // The dispatch entry point + one action function per action.
    assert!(content.contains("def process_action("), "got:\n{content}");
    for action in &["def index(", "def show(", "def create(", "def destroy("] {
        assert!(content.contains(action), "missing {action} in:\n{content}");
    }
}

#[test]
fn routes_table_lists_routes() {
    let app = analyzed_app();
    let files = elixir::emit(&app);
    let content = find(&files, "lib/routes_table.ex");
    assert!(content.contains("defmodule RoutesTable do"), "got:\n{content}");
    // Each route is a Router.Route struct with verb / path / controller /
    // action (the controller is the string name the dispatch shim maps).
    assert!(
        content.contains(
            "ActionDispatch.Router.Route.new(\"GET\", \"/posts\", \"PostsController\", :index)"
        ),
        "got:\n{content}"
    );
}

#[test]
fn save_primary_key_guards_are_folded_before_functionalization() {
    let mut app = roundhouse::App::default();
    app.schema = roundhouse::ingest::ingest_schema(
        b"ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.integer :value\n  end\n  create_table :taggings, id: false do |t|\n    t.integer :tag_id\n  end\nend\n",
        "db/schema.rb",
    ).unwrap();
    for name in ["Widget", "Tagging"] {
        let source = format!("class {name} < ActiveRecord::Base\nend\n");
        app.models.push(roundhouse::ingest::ingest_model(
            source.as_bytes(), &format!("app/models/{}.rb", name.to_lowercase()),
            &app.schema, &Default::default(),
        ).unwrap().unwrap());
    }
    let files = elixir::emit(&app);
    for (file, keyed) in [("lib/widget.ex", true), ("lib/tagging.ex", false)] {
        let source = find(&files, file);
        let save = source.split("  def save_after_validation(record) do\n").nth(1).expect("save body")
            .split("\n  def ").next().unwrap();
        assert!(!save.contains("is_nil(primary_key())"), "{file}: {save}");
        assert_eq!(save.matches("_adapter_insert(record)").count(), 1, "{file}: {save}");
        assert_eq!(save.contains("id: inserted_key"), keyed, "{file}: {save}");
        if !keyed {
            assert!(!save.contains("inserted_key"), "unused binding: {save}");
        }
    }
}

/// Elixir has no `while`: `while_to_recursion` turns a counter loop into a
/// recursive helper, and a loop outside its shape is emitted as a `raise`.
/// The shared runtime's view helpers all take that shape.
#[test]
fn view_helper_loops_lower_to_recursion() {
    let mut app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = elixir::emit(&app);
    let helpers = find(&files, "lib/view_helpers.ex");
    assert!(!helpers.contains("While not supported"), "{}", helpers.lines().filter(|l| l.contains("not supported")).collect::<Vec<_>>().join("\n"));
}
