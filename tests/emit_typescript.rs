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
    // One file per model under app/models/, Rails-layout style.
    assert!(paths.contains(&"app/models/post.ts".to_string()), "got {paths:?}");
    assert!(paths.contains(&"app/models/comment.ts".to_string()), "got {paths:?}");
    // Controllers + routes still flat for now — upgraded in later Phase 3 commits.
    assert!(paths.contains(&"src/controllers.ts".to_string()), "got {paths:?}");
    assert!(paths.contains(&"src/routes.ts".to_string()), "got {paths:?}");
}

#[test]
fn models_extend_application_record() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "app/models/post.ts");
    // The import list starts with ApplicationRecord; associations add
    // more symbols (checked separately). Test with a prefix match.
    assert!(
        content.contains("import { ApplicationRecord"),
        "got:\n{content}"
    );
    assert!(content.contains("} from \"juntos\";"), "got:\n{content}");
    assert!(
        content.contains("export class Post extends ApplicationRecord {"),
        "got:\n{content}"
    );
}

#[test]
fn models_declare_static_table_name_and_columns() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "app/models/post.ts");
    assert!(content.contains("static table_name = \"posts\""), "got:\n{content}");
    // `id` is excluded — Juntos handles the primary key universally.
    assert!(content.contains("static columns = [\"title\"];"), "got:\n{content}");
    assert!(!content.contains("\"id\""), "columns must omit id, got:\n{content}");
}

#[test]
fn models_omit_instance_field_declarations() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "app/models/post.ts");
    // Juntos materializes column accessors at runtime; declaring
    // `title: string;` on the class would shadow them. The scaffold
    // version did emit these — Phase 3 drops them.
    assert!(!content.contains("title: string;"), "should not declare fields, got:\n{content}");
    assert!(!content.contains("id: number;"), "should not declare id, got:\n{content}");
}

#[test]
fn model_validations_emit_as_validate_method() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    // tiny-blog's Post has `validates :title, presence: true`.
    let content = find(&files, "app/models/post.ts");
    assert!(content.contains("validate() {"), "got:\n{content}");
    assert!(
        content.contains("this.validates_presence_of(\"title\")"),
        "got:\n{content}"
    );
}

#[test]
fn has_many_association_emits_collection_proxy_getter() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    // tiny-blog's Post has `has_many :comments`.
    let content = find(&files, "app/models/post.ts");
    // Import expands when associations are present.
    assert!(
        content.contains("import { ApplicationRecord, CollectionProxy, modelRegistry } from \"juntos\";"),
        "got:\n{content}"
    );
    // Getter body uses CollectionProxy and looks up target through the registry.
    assert!(content.contains("get comments() {"), "got:\n{content}");
    assert!(content.contains("type: \"has_many\""), "got:\n{content}");
    assert!(content.contains("foreignKey: \"post_id\""), "got:\n{content}");
    assert!(content.contains("modelRegistry.Comment"), "got:\n{content}");
}

#[test]
fn belongs_to_association_emits_reference_getter() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    // tiny-blog's Comment has `belongs_to :post`.
    let content = find(&files, "app/models/comment.ts");
    assert!(
        content.contains("import { ApplicationRecord, Reference, modelRegistry }"),
        "got:\n{content}"
    );
    assert!(content.contains("get post() {"), "got:\n{content}");
    assert!(
        content.contains("new Reference(modelRegistry.Post, this.attributes.post_id)"),
        "got:\n{content}"
    );
}

#[test]
fn optional_belongs_to_emits_ternary_guard() {
    use roundhouse::{
        Association, ClassId, Model, ModelBodyItem, Row, Symbol, TableRef,
    };
    let mut app = roundhouse::App::new();
    app.models.push(Model {
        name: ClassId(Symbol::from("Article")),
        parent: None,
        table: TableRef(Symbol::from("articles")),
        attributes: Row::closed(),
        body: vec![ModelBodyItem::Association {
            assoc: Association::BelongsTo {
                name: Symbol::from("author"),
                target: ClassId(Symbol::from("User")),
                foreign_key: Symbol::from("author_id"),
                optional: true,
            },
            leading_comments: vec![],
            leading_blank_line: false,
        }],
    });
    let files = typescript::emit(&app);
    let content = find(&files, "app/models/article.ts");
    // Optional belongs_to wraps the FK lookup in a ternary.
    assert!(
        content.contains(
            "this.attributes.author_id ? new Reference(modelRegistry.User, this.attributes.author_id) : null"
        ),
        "got:\n{content}"
    );
}

#[test]
fn length_validation_emits_with_options_object() {
    // Construct an ad-hoc model with `validates :body, length: { minimum: 10 }`
    // to exercise the length-rule path (tiny-blog only has presence).
    use roundhouse::{
        ClassId, Model, ModelBodyItem, Row, Symbol, TableRef, Validation, ValidationRule,
    };
    let mut app = roundhouse::App::new();
    app.models.push(Model {
        name: ClassId(Symbol::from("Article")),
        parent: None,
        table: TableRef(Symbol::from("articles")),
        attributes: Row::closed(),
        body: vec![ModelBodyItem::Validation {
            validation: Validation {
                attribute: Symbol::from("body"),
                rules: vec![ValidationRule::Length { min: Some(10), max: None }],
            },
            leading_comments: vec![],
            leading_blank_line: false,
        }],
    });
    let files = typescript::emit(&app);
    let content = find(&files, "app/models/article.ts");
    assert!(
        content.contains("this.validates_length_of(\"body\", {minimum: 10})"),
        "got:\n{content}"
    );
}

#[test]
fn model_methods_emit_with_return_types() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "app/models/post.ts");
    // `normalize_title` in tiny-blog Post; analyzer types it as string.
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
