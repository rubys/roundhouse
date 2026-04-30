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
fn views_emit_as_string_returning_functions() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    // tiny-blog has one view: app/views/posts/index.html.erb.
    let content = find(&files, "app/views/posts/index.html.ts");
    // Function signature: typed single positional arg (model
    // collection), returns string. Controllers call
    // `Views.renderPostsIndex(records)` passing a `Post[]`.
    assert!(
        content.contains("export function renderPostsIndex(posts: Post[]): string {"),
        "got:\n{content}"
    );
    // Text chunks append as string literals. Buffer name is `io`
    // — matches the lowered-IR shape (`io = String.new ; io << ...`)
    // that view_to_library produces, which the thin TS view emitter
    // renders verbatim. The prior `_buf` name was a TS-specific
    // rename in the deleted derivation path.
    assert!(
        content.contains("io += \"<h1>Posts</h1>\\n\";"),
        "got:\n{content}"
    );
    // `<% @posts.each do |post| %>` → JS `for…of`.
    assert!(content.contains("for (const post of posts) {"), "got:\n{content}");
    // Tail is a `return io;`.
    assert!(content.contains("return io;"), "got:\n{content}");
}

#[test]
fn emits_expected_files() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let paths: Vec<_> = files.iter().map(|f| f.path.display().to_string()).collect();
    // One file per model under app/models/, Rails-layout style.
    assert!(paths.contains(&"app/models/post.ts".to_string()), "got {paths:?}");
    assert!(paths.contains(&"app/models/comment.ts".to_string()), "got {paths:?}");
    // One file per controller under app/controllers/.
    assert!(
        paths.contains(&"app/controllers/posts_controller.ts".to_string()),
        "got {paths:?}"
    );
    // One file per view under app/views/, keeping the Rails
    // controller/action.format path shape.
    assert!(
        paths.contains(&"app/views/posts/index.html.ts".to_string()),
        "got {paths:?}"
    );
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
    // Annotated `: string[]` so the literal isn't inferred as a
    // readonly tuple type (which broke variance against
    // ApplicationRecord's static side).
    assert!(
        content.contains("static columns: string[] = [\"title\"];"),
        "got:\n{content}"
    );
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
    // Every action takes an ActionContext (underscore-prefixed when
    // unused by the action body) and returns Promise<ActionResponse>.
    assert!(
        content.contains("export async function index(_context: ActionContext): Promise<ActionResponse>")
            || content.contains("export async function index(context: ActionContext): Promise<ActionResponse>"),
        "got:\n{content}"
    );
    assert!(
        content.contains("export async function show(context: ActionContext): Promise<ActionResponse>"),
        "got:\n{content}"
    );
    assert!(
        content.contains("export async function create(context: ActionContext): Promise<ActionResponse>"),
        "got:\n{content}"
    );
    assert!(
        content.contains("export async function destroy(context: ActionContext): Promise<ActionResponse>"),
        "got:\n{content}"
    );
}

#[test]
fn controller_ivar_writes_become_let_rebinds() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/posts_controller.ts");
    // Walker path: `@posts = Post.all` becomes `const posts =
    // Post.all();` — ivar-to-local rewrite preserves the ivar's
    // name, and the query chain collapses to `.all()` against the
    // Juntos runtime. (The prior scaffold template hardcoded a
    // `records` local; the walker tracks the real name so the
    // implicit-render path can thread it into the view fn.)
    assert!(
        content.contains("const posts = Post.all();"),
        "got:\n{content}"
    );
    assert!(!content.contains("@posts"), "should drop @:\n{content}");
}

#[test]
fn controller_params_bracket_access_rewrites_to_context() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/posts_controller.ts");
    // `params[:id]` lowers via rewrite_for_controller to
    // `context.params.id`, which the walker inlines directly into
    // the `.find(...)` call. The prior scaffold template introduced
    // an intermediate `const id = Number(...)` binding; the walker
    // doesn't, but the runtime behavior matches.
    assert!(
        content.contains("context.params.id"),
        "got:\n{content}"
    );
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
        content.contains("export async function $new(_context: ActionContext)")
            || content.contains("export async function $new(context: ActionContext)"),
        "got:\n{content}"
    );
}

#[test]
fn custom_action_body_walks_through_sendkind_dispatch() {
    // A custom (non-scaffold) action name — anything not in {index,
    // show, new, edit, create, update, destroy} — lowers as
    // ActionKind::Unknown, which routes through the new AST-walker
    // emit path instead of the seven hand-coded scaffold templates.
    // Covers: Send classification (Render / RedirectTo / ModelFind /
    // ModelNew / PathOrUrlHelper), ivar-write rewrite to `const`
    // binding, and the implicit-render fallback.
    use roundhouse::{
        Action, ClassId, Controller, ControllerBodyItem, EffectSet, Expr, ExprNode,
        Literal, RenderTarget, Row, Symbol,
    };
    use roundhouse::span::Span;
    fn sym(s: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Sym { value: Symbol::from(s) },
            },
        )
    }
    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: false,
            },
        )
    }
    let mut app = roundhouse::App::new();
    // Controller name `ArticlesController` → resource `article`,
    // model class `Article` — the walker uses these to synthesize
    // `Views.renderArticlesShow(...)` and `routeHelpers.articlePath`.
    app.controllers.push(Controller {
        name: ClassId(Symbol::from("ArticlesController")),
        parent: None,
        body: vec![
            ControllerBodyItem::Action {
                action: Action {
                    name: Symbol::from("preview"),
                    params: Row::closed(),
                    // Body: `render :show`
                    body: send(None, "render", vec![sym("show")]),
                    renders: RenderTarget::Inferred,
                    effects: EffectSet::pure(),
                },
                leading_comments: vec![],
                leading_blank_line: false,
            },
            ControllerBodyItem::Action {
                action: Action {
                    name: Symbol::from("archive"),
                    params: Row::closed(),
                    // Body: `redirect_to articles_path`
                    body: send(None, "redirect_to", vec![send(None, "articles_path", vec![])]),
                    renders: RenderTarget::Inferred,
                    effects: EffectSet::pure(),
                },
                leading_comments: vec![],
                leading_blank_line: false,
            },
        ],
    });
    // Model is needed so classify_controller_send treats `Article.*`
    // as a known-model call.
    use roundhouse::Model;
    use roundhouse::ident::TableRef;
    app.models.push(Model {
        name: ClassId(Symbol::from("Article")),
        parent: None,
        table: TableRef(Symbol::from("articles")),
        attributes: Row::closed(),
        body: vec![],
    });
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/articles_controller.ts");
    // `render :show` → typed view fn lookup against the model class.
    // The body has no prior Assign, so the walker passes the
    // undefined-as-any fallback for the view fn's positional arg —
    // a degenerate shape that tsc accepts and that real controllers
    // (which bind a record via before_action or explicit assign)
    // never hit.
    assert!(
        content.contains("return { body: Views.renderArticlesShow(undefined as any) };"),
        "preview body should render via Views.renderArticlesShow, got:\n{content}"
    );
    // `redirect_to articles_path` → PathOrUrlHelper → routeHelpers
    // call site. The helper name is the lower-camel form of the
    // `*_path` method.
    assert!(
        content.contains("return { status: 303, location: routeHelpers.articlesPath() };"),
        "archive body should redirect via routeHelpers.articlesPath, got:\n{content}"
    );
    // No residual 501 stub — the Unknown path no longer short-circuits.
    assert!(
        !content.contains("return { status: 501 }"),
        "Unknown path should walk body, not emit 501 stub, got:\n{content}"
    );
}

#[test]
fn walker_passes_last_bound_local_to_view_fn() {
    // A custom action whose body binds a local before rendering
    // — the walker should pass *that* local to the view fn, not
    // the legacy hardcoded `record`. Proves the last-local state
    // threading works end-to-end; required before the scaffold
    // Show / New / Edit / Index arms can cut over to the walker.
    use roundhouse::{
        Action, ClassId, Controller, ControllerBodyItem, EffectSet, Expr, ExprNode, LValue,
        Literal, RenderTarget, Row, Symbol,
    };
    use roundhouse::ident::{TableRef, VarId};
    use roundhouse::span::Span;
    use roundhouse::Model;
    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: false,
            },
        )
    }
    // Body: `article = Article.find(1); render :show` — the first
    // statement binds `article`, the second terminals on `render`.
    let article_assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("article") },
            value: send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("Article")] },
                )),
                "find",
                vec![Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Int { value: 1 } },
                )],
            ),
        },
    );
    let render_call = send(
        None,
        "render",
        vec![Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from("show") } },
        )],
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Seq { exprs: vec![article_assign, render_call] },
    );

    let mut app = roundhouse::App::new();
    app.models.push(Model {
        name: ClassId(Symbol::from("Article")),
        parent: None,
        table: TableRef(Symbol::from("articles")),
        attributes: Row::closed(),
        body: vec![],
    });
    app.controllers.push(Controller {
        name: ClassId(Symbol::from("ArticlesController")),
        parent: None,
        body: vec![ControllerBodyItem::Action {
            action: Action {
                name: Symbol::from("pinned"),
                params: Row::closed(),
                body,
                renders: RenderTarget::Inferred,
                effects: EffectSet::pure(),
            },
            leading_comments: vec![],
            leading_blank_line: false,
        }],
    });
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/articles_controller.ts");
    // Walker threads `article` (the locally-bound name) into the
    // view fn call — *not* the scaffold's legacy `record`.
    assert!(
        content.contains("Views.renderArticlesShow(article)"),
        "walker should pass last-bound local `article`, got:\n{content}"
    );
    // And sanity: the Assign emitted as a `const` binding.
    assert!(
        content.contains("const article = (Article.find(1) ?? new Article());"),
        "expected const binding for `article`, got:\n{content}"
    );
}

#[test]
fn custom_action_with_respond_to_flattens_to_html_branch() {
    // A custom action whose body is `respond_to { format.html {
    // redirect_to articles_path } format.json { head } }` — the
    // unwrap_respond_to lowering pass flattens this to just the
    // redirect, which the walker then renders as a 303.
    use roundhouse::{
        Action, ClassId, Controller, ControllerBodyItem, EffectSet, Expr, ExprNode,
        RenderTarget, Row, Symbol,
    };
    use roundhouse::ident::{TableRef, VarId};
    use roundhouse::span::Span;
    use roundhouse::Model;
    fn lambda(body: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lambda {
                params: vec![],
                block_param: None,
                body,
                block_style: roundhouse::expr::BlockStyle::Do,
            },
        )
    }
    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: false,
            },
        )
    }
    fn send_with_block(recv: Option<Expr>, method: &str, block: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args: vec![],
                block: Some(block),
                parenthesized: false,
            },
        )
    }
    let format_var = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(0), name: Symbol::from("format") },
    );
    let html_call = send_with_block(
        Some(format_var.clone()),
        "html",
        lambda(send(None, "redirect_to", vec![send(None, "articles_path", vec![])])),
    );
    let json_call = send_with_block(
        Some(format_var),
        "json",
        lambda(send(None, "head", vec![])),
    );
    let pair = Expr::new(
        Span::synthetic(),
        ExprNode::Seq { exprs: vec![html_call, json_call] },
    );
    let body = send_with_block(None, "respond_to", lambda(pair));

    let mut app = roundhouse::App::new();
    app.models.push(Model {
        name: ClassId(Symbol::from("Article")),
        parent: None,
        table: TableRef(Symbol::from("articles")),
        attributes: Row::closed(),
        body: vec![],
    });
    app.controllers.push(Controller {
        name: ClassId(Symbol::from("ArticlesController")),
        parent: None,
        body: vec![ControllerBodyItem::Action {
            action: Action {
                name: Symbol::from("archive_all"),
                params: Row::closed(),
                body,
                renders: RenderTarget::Inferred,
                effects: EffectSet::pure(),
            },
            leading_comments: vec![],
            leading_blank_line: false,
        }],
    });
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/articles_controller.ts");
    // html-branch redirect lifted out of the respond_to; json dropped.
    assert!(
        content.contains("return { status: 303, location: routeHelpers.articlesPath() };"),
        "respond_to html branch should emit as redirect, got:\n{content}"
    );
    // No unreachable-stub comment — the pass flattened successfully.
    assert!(
        !content.contains("unreachable: respond_to"),
        "respond_to should have been flattened, got:\n{content}"
    );
    // No TODO leftover from the old walker stub.
    assert!(
        !content.contains("TODO: respond_to"),
        "old respond_to TODO should be gone, got:\n{content}"
    );
}

#[test]
fn custom_action_without_terminal_gets_implicit_render() {
    // A custom action body with no explicit render/redirect_to/head
    // should get a synthesized `render :<action>` appended by the
    // `synthesize_implicit_render` lowering pass. Verifies the pass
    // is threaded into the TS walker path — the emitted function
    // returns Views.renderArticlesHeadline(record), not a fallback
    // stub.
    use roundhouse::{
        Action, ClassId, Controller, ControllerBodyItem, EffectSet, Expr, ExprNode,
        Literal, RenderTarget, Row, Symbol,
    };
    use roundhouse::span::Span;
    use roundhouse::ident::TableRef;
    use roundhouse::Model;
    let mut app = roundhouse::App::new();
    app.models.push(Model {
        name: ClassId(Symbol::from("Article")),
        parent: None,
        table: TableRef(Symbol::from("articles")),
        attributes: Row::closed(),
        body: vec![],
    });
    app.controllers.push(Controller {
        name: ClassId(Symbol::from("ArticlesController")),
        parent: None,
        body: vec![ControllerBodyItem::Action {
            action: Action {
                name: Symbol::from("headline"),
                params: Row::closed(),
                // Body: just a literal — no render/redirect/head.
                body: Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Int { value: 42 } },
                ),
                renders: RenderTarget::Inferred,
                effects: EffectSet::pure(),
            },
            leading_comments: vec![],
            leading_blank_line: false,
        }],
    });
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/articles_controller.ts");
    // Synthesized terminal becomes the view fn keyed by action name.
    // No prior Assign → `undefined as any` fallback (same rationale
    // as custom_action_body_walks_through_sendkind_dispatch).
    assert!(
        content.contains("return { body: Views.renderArticlesHeadline(undefined as any) };"),
        "implicit render should synthesize Views.renderArticlesHeadline, got:\n{content}"
    );
    // No empty-body fallback — the lowering pass makes it unnecessary.
    assert!(
        !content.contains("return { body: \"\" };"),
        "fallback return should be gone, got:\n{content}"
    );
}

#[test]
fn routes_emit_router_method_calls() {
    // Use the real-blog fixture so we exercise `root` +
    // `resources` + nested resources — tiny-blog is all explicit verbs.
    let mut app = roundhouse::ingest::ingest_app(
        std::path::Path::new("fixtures/real-blog"),
    )
    .expect("ingest real-blog");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let files = typescript::emit(&app);
    let content = find(&files, "src/routes.ts");

    // Router + controller imports at the top.
    assert!(
        content.contains("import { Router } from \"juntos\";"),
        "got:\n{content}"
    );
    assert!(
        content.contains(
            "import { ArticlesController } from \"../app/controllers/articles_controller.js\";"
        ),
        "got:\n{content}"
    );
    assert!(
        content.contains(
            "import { CommentsController } from \"../app/controllers/comments_controller.js\";"
        ),
        "got:\n{content}"
    );

    // `root "articles#index"` → Router.root("/", ArticlesController, "index").
    assert!(
        content.contains("Router.root(\"/\", ArticlesController, \"index\");"),
        "got:\n{content}"
    );

    // Top-level resources + nested resources.
    assert!(
        content.contains(
            "Router.resources(\"articles\", ArticlesController, {nested: [{name: \"comments\", controller: CommentsController, only: [\"create\", \"destroy\"]}]});"
        ),
        "got:\n{content}"
    );
}

#[test]
fn controllers_export_namespace_object() {
    let app = analyzed_app();
    let files = typescript::emit(&app);
    let content = find(&files, "app/controllers/posts_controller.ts");
    // Router dispatches via `PostsController.<action>`; controller file
    // exports this namespace object alongside the individual functions.
    assert!(
        content.contains("export const PostsController = { index, show, create, destroy };"),
        "got:\n{content}"
    );
}

// Adapter-driven await insertion -----------------------------------------
//
// Under the default `SqliteAdapter`, nothing suspends — emitted TS is
// unchanged from the pre-consumption behavior (the `async function`
// wrapper remains but no `await` inside action bodies). Under
// `SqliteAsyncAdapter`, DB-touching Sends pick up `await` at statement-
// level expression sites: RHS of an assign, condition of an if, plain
// expression statements. Non-DB effects (Io from `redirect_to` / `render`)
// don't suspend under this adapter (SqliteAsync is DB-scoped) so those
// Sends stay unawaited.

#[test]
fn sync_adapter_omits_awaits_inside_action_bodies() {
    let app = analyzed_app();
    let files = typescript::emit_with_adapter(&app, &roundhouse::SqliteAdapter);
    let content = find(&files, "app/controllers/posts_controller.ts");
    // The `async function` wrappers remain — they were there before
    // adapters existed. What should be absent is any `await` inside
    // the body under the sync adapter.
    for action in ["index", "show", "destroy", "create"] {
        let idx = content
            .find(&format!("export async function {action}"))
            .unwrap_or_else(|| panic!("expected {action} function in:\n{content}"));
        let after = &content[idx..];
        let end = after.find("\n}").unwrap_or(after.len());
        let body = &after[..end];
        assert!(
            !body.contains(" await "),
            "sync adapter should not emit `await` in {action} body:\n{body}",
        );
    }
}

#[test]
fn async_adapter_awaits_model_find_on_assign_rhs() {
    // `@post = Post.find(params[:id])` — the TS ModelFind render
    // produces a nullable-coalesce: `(Post.find(id) ?? new Post())`.
    // Under SqliteAsyncAdapter, `await` must land on the
    // `Post.find(id)` sub-expression, NOT the whole coalesce.
    // Correct shape:  `(await Post.find(id) ?? new Post())`
    // which parses as `((await Post.find(id)) ?? new Post())` by
    // precedence (await=17, ??=3).
    //
    // The wrong shape `await (Post.find(id) ?? new Post())`
    // would await a Promise-or-new-Post-coalesce: since Promise
    // is truthy, `??` returns the Promise unchanged and the
    // outer await resolves it — dropping the fallback path.
    let app = analyzed_app();
    let files = typescript::emit_with_adapter(&app, &roundhouse::SqliteAsyncAdapter);
    let content = find(&files, "app/controllers/posts_controller.ts");
    assert!(
        content.contains("(await Post.find("),
        "expected `(await Post.find(...` inside the coalesce in:\n{content}",
    );
    // Negative: the wrong outer-await shape must not appear.
    assert!(
        !content.contains("await (Post.find("),
        "`await` must land on the find, not on the whole coalesce:\n{content}",
    );
}

#[test]
fn async_adapter_awaits_model_all_on_assign_rhs() {
    // `@posts = Post.all` → `const posts = await Post.all();`
    let app = analyzed_app();
    let files = typescript::emit_with_adapter(&app, &roundhouse::SqliteAsyncAdapter);
    let content = find(&files, "app/controllers/posts_controller.ts");
    assert!(
        content.contains("const posts = await Post.all()"),
        "expected `const posts = await Post.all()` in:\n{content}",
    );
}

#[test]
fn async_adapter_awaits_destroy_as_statement() {
    // `@post.destroy` as a bare statement → `await post.destroy;`
    // under SqliteAsyncAdapter. The Send is DbWrite; the adapter
    // flags it as suspending; the Send-at-statement path in
    // `walk_stmt` prepends the walker's `suspending_prefix`.
    let app = analyzed_app();
    let files = typescript::emit_with_adapter(&app, &roundhouse::SqliteAsyncAdapter);
    let content = find(&files, "app/controllers/posts_controller.ts");
    assert!(
        content.contains("await post.destroy"),
        "expected `await post.destroy` in:\n{content}",
    );
}

#[test]
fn async_adapter_does_not_await_non_db_sends() {
    // `redirect_to posts_path` carries `Io`, not `DbRead`/`DbWrite`.
    // SqliteAsyncAdapter suspends only DB effects, so `Io`-bearing
    // sends stay unawaited even under the async adapter. This is the
    // intended scoping — `await` tracks DB-specific suspension, not
    // universal "has effect" semantics.
    let app = analyzed_app();
    let files = typescript::emit_with_adapter(&app, &roundhouse::SqliteAsyncAdapter);
    let content = find(&files, "app/controllers/posts_controller.ts");
    // Find every `redirect_to`-style return and confirm no `await`
    // prefix. (The scaffold destroy action uses a bare `return`
    // with the path helper; we look for `return {` patterns in the
    // response-shape emission.)
    assert!(
        !content.contains("await return"),
        "redirect responses should not gain `await` under SqliteAsync:\n{content}",
    );
    // Also verify: the `return { body:` / `return { status:`
    // response fragments stay plain.
    for line in content.lines() {
        if line.contains("return {") {
            assert!(
                !line.contains("await "),
                "response statement should not be awaited: `{line}`",
            );
        }
    }
}

#[test]
fn async_adapter_places_await_at_suspending_subexpression() {
    // Stronger form of the find-coalesce test: verify every
    // suspending Send in the generated TS has its `await`
    // positioned correctly (before the Send, inside any compound
    // wrapping) — not at an outer operator position where it
    // would await the compound result instead of the Promise.
    //
    // Specifically: no line should contain `await (` followed by
    // a non-await identifier that's not immediately a Send. The
    // pattern `await (X ??` or `await (X &&` would indicate the
    // outer-wrap bug.
    let app = analyzed_app();
    let files = typescript::emit_with_adapter(&app, &roundhouse::SqliteAsyncAdapter);
    let content = find(&files, "app/controllers/posts_controller.ts");
    for line in content.lines() {
        // `await (<identifier><space>??` would be the outer-wrap
        // bug. Scan for it.
        if line.contains("await (") && line.contains(" ?? ") {
            let outer_start = line.find("await (").unwrap();
            let coalesce = line.find(" ?? ").unwrap();
            assert!(
                outer_start > coalesce,
                "`await (X ?? Y)` pattern suggests outer-wrap bug; await should be inside:\n{line}",
            );
        }
    }
}

#[test]
fn library_class_emits_as_plain_ts_class() {
    // transpiled_blog has ArticleCommentsProxy as a non-model class
    // under app/models/. The TS emitter should produce a plain
    // `export class ArticleCommentsProxy { ... }` — no
    // ApplicationRecord parent, no modelRegistry registration.
    let mut app = ingest_app(Path::new("runtime/ruby/test/fixtures/transpiled_blog"))
        .expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = typescript::emit(&app);
    let content = find(&files, "app/models/article_comments_proxy.ts");

    assert!(
        content.contains("export class ArticleCommentsProxy {"),
        "expected plain class declaration, got:\n{content}"
    );
    assert!(
        !content.contains("extends ApplicationRecord"),
        "library class should not extend ApplicationRecord:\n{content}"
    );
    assert!(
        !content.contains("modelRegistry["),
        "library class should not register in modelRegistry:\n{content}"
    );
    // Methods are present as walked bodies (correctness of body
    // emission is tracked by separate work). `initialize` emits as
    // a TS `constructor` per the Ruby→TS convention.
    for fragment in ["constructor(", "to_a", "size", "build", "create"] {
        assert!(
            content.contains(fragment),
            "expected `{fragment}` in proxy output:\n{content}"
        );
    }
}

#[test]
fn sync_and_async_differ_only_on_awaits() {
    // Byte-diff the two outputs: the only differences should be
    // `await ` insertions. Same actions, same function bodies
    // otherwise.
    let app = analyzed_app();
    let sync = typescript::emit_with_adapter(&app, &roundhouse::SqliteAdapter);
    let async_ = typescript::emit_with_adapter(&app, &roundhouse::SqliteAsyncAdapter);
    let sync_content = find(&sync, "app/controllers/posts_controller.ts");
    let async_content = find(&async_, "app/controllers/posts_controller.ts");

    // Async version must be longer (contains awaits).
    assert!(
        async_content.len() > sync_content.len(),
        "async output should be larger (awaits added)",
    );
    // Stripping every `await ` from the async output should match
    // the sync output byte-for-byte.
    let stripped = async_content.replace("await ", "");
    assert_eq!(
        stripped, sync_content,
        "async - sync difference must be only `await ` insertions",
    );
}
