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
    // One file per controller under app/controllers/, same convention.
    assert!(
        paths.contains(&"app/controllers/posts_controller.ts".to_string()),
        "got {paths:?}"
    );
    // Routes still flat for now — upgraded in the next Phase 3 commit.
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
fn broadcasts_to_emits_turbo_callback_registrations() {
    // real-blog's Article has
    //   broadcasts_to ->(_article) { "articles" }, inserts_by: :prepend
    // We ingest the real-blog fixture directly so the IR shape
    // matches the actual Rails source rather than a hand-built model.
    let mut app = roundhouse::ingest::ingest_app(
        std::path::Path::new("fixtures/real-blog"),
    )
    .expect("ingest real-blog");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let files = typescript::emit(&app);

    // The Article model is emitted to app/models/article.ts.
    let content = find(&files, "app/models/article.ts");

    // inserts_by: :prepend → broadcastPrependTo on save.
    assert!(
        content.contains(
            "Article.afterSave((record) => record.broadcastPrependTo(\"articles\"));"
        ),
        "got:\n{content}"
    );
    // Destroy always removes.
    assert!(
        content.contains(
            "Article.afterDestroy((record) => record.broadcastRemoveTo(\"articles\"));"
        ),
        "got:\n{content}"
    );
}

#[test]
fn broadcasts_to_rewrites_lambda_param_to_record() {
    // real-blog's Comment has
    //   broadcasts_to ->(comment) { "article_#{comment.article_id}_comments" }
    // The lambda param (`comment`) is what's visible to the stream
    // template; in the Juntos callback it's named `record`. Emit must
    // rewrite the reference.
    let mut app = roundhouse::ingest::ingest_app(
        std::path::Path::new("fixtures/real-blog"),
    )
    .expect("ingest real-blog");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let files = typescript::emit(&app);
    let content = find(&files, "app/models/comment.ts");
    assert!(
        content.contains("record.article_id"),
        "expected `record.article_id` rewrite in:\n{content}"
    );
    // Comment uses `target: "comments"` — passed as the opts object
    // second arg to broadcastAppendTo.
    assert!(
        content.contains("{ target: \"comments\" }"),
        "expected target-opts hash in:\n{content}"
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
    // Snake_case preserved for Rails-facing compat with Juntos;
    // `normalize_title` stays `normalize_title`, not `normalizeTitle`.
    assert!(
        content.contains("normalize_title(): string"),
        "got:\n{content}"
    );
}

#[test]
fn controllers_emit_as_module_of_exported_async_functions() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/posts_controller.ts");
    // No class wrapper — each action is a top-level exported function.
    assert!(!content.contains("export class"), "controllers shouldn't emit as classes:\n{content}");
    // Read actions take just `context`; write actions take `context, params`.
    assert!(
        content.contains("export async function index(context)"),
        "got:\n{content}"
    );
    assert!(
        content.contains("export async function show(context)"),
        "got:\n{content}"
    );
    assert!(
        content.contains("export async function create(context, params)"),
        "got:\n{content}"
    );
    assert!(
        content.contains("export async function destroy(context)"),
        "got:\n{content}"
    );
}

#[test]
fn controller_ivar_writes_become_let_rebinds() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/posts_controller.ts");
    // `@posts = Post.all` → `let posts = Post.all;`. The `@` dies.
    assert!(content.contains("let posts = Post.all;"), "got:\n{content}");
    assert!(!content.contains("@posts"), "should drop @:\n{content}");
}

#[test]
fn controller_params_bracket_access_rewrites_to_context() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/posts_controller.ts");
    // `params[:id]` → `context.params.id`.
    assert!(
        content.contains("Post.find(context.params.id)"),
        "got:\n{content}"
    );
}

#[test]
fn controller_new_action_is_reserved_word_escaped() {
    // Build a minimal controller with a `new` action since tiny-blog
    // doesn't have one. `new` is reserved in JS; ruby2js renames to `$new`.
    use roundhouse::{
        Action, ClassId, Controller, ControllerBodyItem, EffectSet, Expr, ExprNode, RenderTarget,
        Row, Symbol,
    };
    use roundhouse::span::Span;
    let mut app = roundhouse::App::new();
    app.controllers.push(Controller {
        name: ClassId(Symbol::from("WidgetsController")),
        parent: None,
        body: vec![ControllerBodyItem::Action {
            action: Action {
                name: Symbol::from("new"),
                params: Row::closed(),
                body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
                renders: RenderTarget::Inferred,
                effects: EffectSet::pure(),
            },
            leading_comments: vec![],
            leading_blank_line: false,
        }],
    });
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/widgets_controller.ts");
    assert!(
        content.contains("export async function $new(context)"),
        "got:\n{content}"
    );
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
