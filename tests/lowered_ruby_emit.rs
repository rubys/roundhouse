//! Regression test for the lower → Ruby emit pipeline. Drives
//! `emit_lowered_models` against `fixtures/real-blog` and asserts the
//! emitted source matches the universal post-lowering shape. The hand-
//! written `fixtures/spinel-blog/app/models/*.rb` is the visual
//! reference; this test asserts structural equivalents (key methods
//! present with the right body shapes) rather than byte-for-byte match,
//! so surface-formatting churn doesn't ripple in.
//!
//! See `project_lowerers_first_validate_via_spinel.md` — Spinel is the
//! validation target for the lowering pipeline; per-target emitter
//! migrations (TS / Rust / …) are deferred.

use roundhouse::emit::{ruby, EmittedFile};
use roundhouse::ingest::ingest_app;

fn lowered_real_blog() -> Vec<EmittedFile> {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    ruby::emit_lowered_models(&app)
}

fn lowered_real_blog_schema() -> String {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    ruby::emit_lowered_schema(&app).content
}

fn lowered_real_blog_routes() -> String {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    ruby::emit_lowered_routes(&app).content
}

fn lowered_real_blog_controllers() -> Vec<EmittedFile> {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    ruby::emit_lowered_controllers(&app)
}

fn find<'a>(files: &'a [EmittedFile], suffix: &str) -> &'a str {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.as_str())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {suffix}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

#[test]
fn one_file_per_model() {
    let files = lowered_real_blog();
    let names: Vec<String> = files
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();
    assert!(names.iter().any(|n| n.ends_with("article.rb")), "{names:?}");
    assert!(names.iter().any(|n| n.ends_with("comment.rb")), "{names:?}");
    assert!(
        names.iter().any(|n| n.ends_with("application_record.rb")),
        "{names:?}",
    );
}

#[test]
fn application_record_renders_abstract_marker() {
    let files = lowered_real_blog();
    let src = find(&files, "application_record.rb");
    assert!(src.contains("class ApplicationRecord < ActiveRecord::Base"), "{src}");
    // `primary_abstract_class` lowers to `def self.abstract?; true; end`
    // — the explicit form spinel-blog uses.
    assert!(src.contains("def self.abstract?"), "{src}");
}

#[test]
fn article_renders_schema_scaffold_methods() {
    let files = lowered_real_blog();
    let src = find(&files, "article.rb");
    assert!(src.contains("class Article < ApplicationRecord"), "{src}");
    for m in [
        "def title",
        "def body",
        "def created_at",
        "def updated_at",
        "def self.table_name",
        "def self.schema_columns",
        "def self.instantiate(row)",
        "def initialize(attrs)",
        "def attributes",
        "def [](name)",
        "def []=(name, value)",
        "def update(attrs)",
    ] {
        assert!(src.contains(m), "missing `{m}`:\n{src}");
    }
}

#[test]
fn article_renders_validate_with_block_helpers() {
    let files = lowered_real_blog();
    let src = find(&files, "article.rb");
    assert!(src.contains("def validate"), "{src}");
    // Validates_*_of helpers carry block-yielding @attr access — the
    // shape spinel's runtime expects.
    assert!(
        src.contains("validates_presence_of(:title) { @title }"),
        "{src}",
    );
    assert!(
        src.contains("validates_length_of(:body, minimum: 10)"),
        "{src}",
    );
}

#[test]
fn article_renders_has_many_reader_and_dependent_destroy() {
    let files = lowered_real_blog();
    let src = find(&files, "article.rb");
    assert!(
        src.contains("def comments") && src.contains("Comment.where(article_id: @id)"),
        "{src}",
    );
    assert!(
        src.contains("def before_destroy") && src.contains("comments.each"),
        "{src}",
    );
}

#[test]
fn comment_renders_belongs_to_with_fk_guard() {
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    assert!(src.contains("class Comment < ApplicationRecord"), "{src}");
    assert!(src.contains("def article"), "{src}");
    assert!(src.contains("Article.find_by(id: @article_id)"), "{src}");
}

#[test]
fn equality_send_renders_as_infix() {
    // belongs_to lowering produces `if @article_id == 0` as an If
    // whose cond is a Send `==`. emit_send_base renders Send's
    // operator-named methods as infix syntax.
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    assert!(
        src.contains("@article_id == 0"),
        "expected infix `==`; emit_send_base regression?\n{src}",
    );
    assert!(
        !src.contains("@article_id.=="),
        "infix should not render as method-call form; got:\n{src}",
    );
}

#[test]
fn article_broadcasts_to_synthesizes_three_lifecycle_methods() {
    // Article has `broadcasts_to ->(_article) { "articles" },
    // inserts_by: :prepend`. Lowered into three methods: prepend on
    // create (per inserts_by), replace on update, remove on destroy.
    // Channel is the literal "articles"; per-record target falls
    // back to "article_#{@id}" on update + destroy.
    let files = lowered_real_blog();
    let src = find(&files, "article.rb");
    assert!(
        src.contains(
            "Broadcasts.prepend(stream: \"articles\", target: \"articles\", html: Views::Articles.article(self))"
        ),
        "{src}",
    );
    assert!(
        src.contains(
            "Broadcasts.replace(stream: \"articles\", target: \"article_#{@id}\", html: Views::Articles.article(self))"
        ),
        "{src}",
    );
    assert!(
        src.contains(
            "Broadcasts.remove(stream: \"articles\", target: \"article_#{@id}\")"
        ),
        "{src}",
    );
}

#[test]
fn comment_broadcasts_rewrite_lambda_param_to_ivar() {
    // Comment has `broadcasts_to ->(comment) { "article_#{comment.article_id}_comments" },
    // target: "comments"`. The lambda param `comment` rewrites to
    // ivar references in the expanded body — `comment.article_id`
    // becomes `@article_id`.
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    assert!(
        src.contains("stream: \"article_#{@article_id}_comments\""),
        "expected lambda-param→ivar rewrite; got:\n{src}",
    );
    // create uses the explicit `target: "comments"` override.
    assert!(
        src.contains("Broadcasts.append(stream: \"article_#{@article_id}_comments\", target: \"comments\""),
        "{src}",
    );
    // update uses the canonical per-record target, NOT the override
    // (Rails turbo convention: target: only governs create-time DOM).
    assert!(
        src.contains("Broadcasts.replace(stream: \"article_#{@article_id}_comments\", target: \"comment_#{@id}\""),
        "{src}",
    );
}

#[test]
fn application_record_requires_active_record_runtime() {
    // ApplicationRecord's parent is `ActiveRecord::Base`, which lives
    // in `runtime/active_record.rb`. The require_relative path resolves
    // from `app/models/application_record.rb` up two levels.
    let files = lowered_real_blog();
    let src = find(&files, "application_record.rb");
    assert!(
        src.contains("require_relative \"../../runtime/active_record\""),
        "{src}",
    );
    assert!(!src.contains("require_relative \"application_record\""), "{src}");
}

#[test]
fn article_emits_parent_runtime_and_view_requires() {
    // Article needs:
    //   - parent: ApplicationRecord (same dir)
    //   - Broadcasts (runtime module)
    //   - Views::Articles (view partial)
    // Sibling `Comment` is autoloaded; no require for it.
    let files = lowered_real_blog();
    let src = find(&files, "article.rb");
    assert!(src.contains("require_relative \"application_record\""), "{src}");
    assert!(src.contains("require_relative \"../../runtime/broadcasts\""), "{src}");
    assert!(src.contains("require_relative \"../views/articles/_article\""), "{src}");
    assert!(!src.contains("require_relative \"comment\""), "{src}");
}

#[test]
fn comment_emits_view_require_for_own_partial() {
    // Comment references `Views::Comments` (own partial) via the
    // broadcasts_to expansion's `html:` payload. The cascade-render
    // for the parent Article uses real-blog's literal
    // `article.broadcast_replace_to("articles")` form, which has no
    // Views::Articles reference — so only the comments partial gets
    // a require here. (Spinel-blog rewrites the cascade into an
    // explicit Views::Articles call; per yagni-on-round-trip we keep
    // the literal form, which is compile-equivalent.)
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    assert!(src.contains("require_relative \"../views/comments/_comment\""), "{src}");
    assert!(
        !src.contains("require_relative \"../views/articles/_article\""),
        "Views::Articles isn't referenced in Comment's lowered body; should not require:\n{src}",
    );
}

#[test]
fn comment_broadcasts_compose_with_block_form_callback() {
    // Comment has both `broadcasts_to` AND
    // `after_create_commit { article.broadcast_replace_to(...) rescue nil }`.
    // The two sources fold into one method body; broadcasts_to runs
    // first (source order in the lowering), block-form follows.
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    let create_block = src
        .split("def after_create_commit").nth(1).unwrap()
        .split("def ").next().unwrap();
    assert!(
        create_block.contains("Broadcasts.append")
            && create_block.contains("article.broadcast_replace_to(\"articles\") rescue nil"),
        "expected composed body; got:\n{create_block}",
    );
    // The Broadcasts.append call appears BEFORE the rescue line —
    // composition order matches the expected source-order semantics.
    let pos_broadcasts = create_block.find("Broadcasts.append").unwrap();
    let pos_rescue = create_block.find("rescue nil").unwrap();
    assert!(pos_broadcasts < pos_rescue, "{create_block}");
}

#[test]
fn comment_block_callbacks_render_as_methods() {
    // real-blog's Comment has:
    //   after_create_commit { article.broadcast_replace_to("articles") rescue nil }
    //   after_destroy_commit { article.broadcast_replace_to("articles") rescue nil }
    // Lowered to `def after_create_commit; …; end` etc.
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    assert!(src.contains("def after_create_commit"), "{src}");
    assert!(src.contains("def after_destroy_commit"), "{src}");
    // The block body uses `... rescue nil` — RescueModifier must render
    // surface-form as `expr rescue nil`.
    assert!(
        src.contains("article.broadcast_replace_to(\"articles\") rescue nil"),
        "expected RescueModifier surface form; got:\n{src}",
    );
}

#[test]
fn schema_emits_module_wrapper_at_config_path() {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    let f = ruby::emit_lowered_schema(&app);
    assert_eq!(f.path.to_string_lossy(), "config/schema.rb");
    assert!(f.content.starts_with("module Schema\n"), "{}", f.content);
    assert!(f.content.contains("STATEMENTS = ["), "{}", f.content);
    assert!(f.content.contains("].freeze"), "{}", f.content);
    assert!(f.content.contains("def self.load!(adapter)"), "{}", f.content);
    assert!(
        f.content.contains("STATEMENTS.each { |sql| adapter.execute_ddl(sql) }"),
        "{}",
        f.content,
    );
}

#[test]
fn schema_emits_one_create_table_heredoc_per_table() {
    let src = lowered_real_blog_schema();
    // Each table renders as a `<<~SQL,` heredoc with the canonical
    // SQLite scaffold: id INTEGER PRIMARY KEY AUTOINCREMENT, then
    // each non-PK column with its SQL type and NOT NULL marker.
    assert!(src.contains("CREATE TABLE IF NOT EXISTS articles ("), "{src}");
    assert!(src.contains("CREATE TABLE IF NOT EXISTS comments ("), "{src}");
    let heredoc_count = src.matches("<<~SQL,").count();
    assert_eq!(heredoc_count, 2, "expected one heredoc per table; got:\n{src}");
}

#[test]
fn schema_renders_pk_and_typed_columns() {
    let src = lowered_real_blog_schema();
    // Every table starts with the synthesized id PK line.
    assert!(src.contains("id INTEGER PRIMARY KEY AUTOINCREMENT,"), "{src}");
    // Real-blog's articles table: title (string) → TEXT, body (text)
    // → TEXT, created_at/updated_at (datetime, NOT NULL) → TEXT NOT NULL.
    assert!(src.contains("title TEXT"), "{src}");
    assert!(src.contains("body TEXT"), "{src}");
    assert!(src.contains("created_at TEXT NOT NULL"), "{src}");
    assert!(src.contains("updated_at TEXT NOT NULL"), "{src}");
    // comments.article_id is a NOT NULL integer FK.
    assert!(src.contains("article_id INTEGER NOT NULL"), "{src}");
    assert!(src.contains("commenter TEXT"), "{src}");
}

#[test]
fn schema_emits_create_index_lines_for_table_indexes() {
    let src = lowered_real_blog_schema();
    // comments has `t.index ["article_id"], name: "index_comments_on_article_id"`.
    assert!(
        src.contains(
            "\"CREATE INDEX IF NOT EXISTS index_comments_on_article_id ON comments (article_id)\","
        ),
        "{src}",
    );
}

#[test]
fn schema_drops_foreign_key_constraints() {
    // Real-blog has `add_foreign_key "comments", "articles"` — but the
    // spinel runtime models relationships at the app layer (e.g.
    // belongs_to lowers to `Article.find_by(id: @article_id)`), so the
    // DB-level constraint is dropped per yagni-on-round-trip:
    // structural compile-equivalence, not source-equivalence.
    let src = lowered_real_blog_schema();
    assert!(
        !src.contains("FOREIGN KEY") && !src.contains("REFERENCES"),
        "spinel schema should not emit FK constraints; got:\n{src}",
    );
}

#[test]
fn routes_emits_module_wrapper_at_config_path() {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    let f = ruby::emit_lowered_routes(&app);
    assert_eq!(f.path.to_string_lossy(), "config/routes.rb");
    assert!(f.content.contains("module Routes"), "{}", f.content);
    assert!(f.content.contains("TABLE = ["), "{}", f.content);
    assert!(f.content.contains("].freeze"), "{}", f.content);
}

#[test]
fn routes_require_application_controller_plus_each_referenced_controller() {
    let src = lowered_real_blog_routes();
    // application_controller is always required at the top — main.rb's
    // dispatch base, included even when no controller in the route table
    // happens to reference it.
    assert!(
        src.contains("require_relative \"../app/controllers/application_controller\""),
        "{src}",
    );
    // Each unique controller used by the table is required exactly once.
    assert!(
        src.contains("require_relative \"../app/controllers/articles_controller\""),
        "{src}",
    );
    assert!(
        src.contains("require_relative \"../app/controllers/comments_controller\""),
        "{src}",
    );
    let articles_count = src.matches("articles_controller\"").count();
    assert_eq!(articles_count, 1, "duplicate require:\n{src}");
}

#[test]
fn routes_table_expands_resources_block_into_concrete_entries() {
    // `resources :articles` expands to 7 entries (index, new, create,
    // show, edit, update, destroy). The spinel TABLE hash form is
    // `{ method: "VERB", pattern: "/path", controller: :sym, action: :sym }`.
    let src = lowered_real_blog_routes();
    for line in [
        r#"{ method: "GET", pattern: "/articles", controller: :articles, action: :index }"#,
        r#"{ method: "GET", pattern: "/articles/new", controller: :articles, action: :new }"#,
        r#"{ method: "POST", pattern: "/articles", controller: :articles, action: :create }"#,
        r#"{ method: "GET", pattern: "/articles/:id", controller: :articles, action: :show }"#,
        r#"{ method: "GET", pattern: "/articles/:id/edit", controller: :articles, action: :edit }"#,
        r#"{ method: "PATCH", pattern: "/articles/:id", controller: :articles, action: :update }"#,
        r#"{ method: "DELETE", pattern: "/articles/:id", controller: :articles, action: :destroy }"#,
    ] {
        assert!(src.contains(line), "missing route entry:\n  {line}\nin:\n{src}");
    }
}

#[test]
fn routes_nest_child_resource_under_parent_id_scope() {
    // `resources :articles do resources :comments, only: [:create, :destroy]`
    // nests under `/articles/:article_id/comments`. only:[] filters to
    // create + destroy; index/new/show/edit/update are dropped.
    let src = lowered_real_blog_routes();
    assert!(
        src.contains(r#"{ method: "POST", pattern: "/articles/:article_id/comments", controller: :comments, action: :create }"#),
        "{src}",
    );
    assert!(
        src.contains(r#"{ method: "DELETE", pattern: "/articles/:article_id/comments/:id", controller: :comments, action: :destroy }"#),
        "{src}",
    );
    // Filtered actions must not appear.
    assert!(
        !src.contains(r#"controller: :comments, action: :index"#),
        "only:[:create, :destroy] should drop :index; got:\n{src}",
    );
    assert!(
        !src.contains(r#"controller: :comments, action: :show"#),
        "{src}",
    );
}

#[test]
fn routes_extract_root_into_separate_constant() {
    // `root "articles#index"` becomes a top-level `ROOT` constant, not
    // a TABLE entry — the spinel router checks ROOT separately so the
    // dispatch loop doesn't have to special-case "/".
    let src = lowered_real_blog_routes();
    assert!(
        src.contains(
            r#"ROOT = { method: "GET", pattern: "/", controller: :articles, action: :index }.freeze"#
        ),
        "{src}",
    );
    // ROOT must NOT also be in TABLE — extracting it is the whole point.
    let table_section = src.split("TABLE = [").nth(1).unwrap()
        .split("].freeze").next().unwrap();
    assert!(
        !table_section.contains("pattern: \"/\""),
        "root should be hoisted out of TABLE; got table:\n{table_section}",
    );
}

#[test]
fn routes_order_literal_segments_before_id_patterns() {
    // Matching semantics: `/articles/new` must appear before
    // `/articles/:id` so the literal-segment match wins. flatten_routes
    // already orders this way (standard_resource_actions has new before
    // show); regression test against future reordering.
    let src = lowered_real_blog_routes();
    let pos_new = src.find(r#"pattern: "/articles/new""#).expect("/articles/new missing");
    let pos_show = src.find(r#"pattern: "/articles/:id", controller: :articles, action: :show"#)
        .expect("/articles/:id show missing");
    assert!(pos_new < pos_show, "literal segment must precede :id pattern");
}

#[test]
fn controllers_one_file_per_controller_at_app_controllers_path() {
    let files = lowered_real_blog_controllers();
    let names: Vec<String> = files.iter().map(|f| f.path.display().to_string()).collect();
    for stem in ["application_controller", "articles_controller", "comments_controller"] {
        assert!(
            names.iter().any(|n| n == &format!("app/controllers/{stem}.rb")),
            "missing {stem}; got {names:?}",
        );
    }
}

#[test]
fn controllers_application_requires_runtime_action_controller() {
    let files = lowered_real_blog_controllers();
    let src = find(&files, "application_controller.rb");
    // Parent is ActionController::Base, which lives in runtime/.
    assert!(src.contains("class ApplicationController < ActionController::Base"), "{src}");
    assert!(
        src.contains("require_relative \"../../runtime/action_controller\""),
        "{src}",
    );
}

#[test]
fn controllers_application_drops_unknown_class_body_calls() {
    // real-blog's ApplicationController has `allow_browser versions: :modern`
    // and `stale_when_importmap_changes` — class-level Sends with no
    // spinel semantics. They get dropped (the lowering only carries
    // filters and actions through; Unknown items disappear).
    let files = lowered_real_blog_controllers();
    let src = find(&files, "application_controller.rb");
    assert!(!src.contains("allow_browser"), "{src}");
    assert!(!src.contains("stale_when_importmap_changes"), "{src}");
}

#[test]
fn controllers_articles_extends_application_controller_in_same_dir() {
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(src.contains("class ArticlesController < ApplicationController"), "{src}");
    // Same-dir parent: bare snake_case, no leading "../" or "./".
    assert!(src.contains("require_relative \"application_controller\""), "{src}");
    assert!(
        !src.contains("require_relative \"../../runtime/action_controller\""),
        "subclass should not directly require the base runtime; got:\n{src}",
    );
}

#[test]
fn controllers_articles_synthesizes_process_action_dispatcher() {
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    // process_action(action_name) is the synthesized entry point —
    // before_action filters become conditional set_X calls; the
    // case dispatch maps action symbols to method calls.
    assert!(src.contains("def process_action(action_name)"), "{src}");
    assert!(src.contains("case action_name"), "{src}");
    for arm in [
        "when :index",
        "when :show",
        "when :new",
        "when :edit",
        "when :create",
        "when :update",
        "when :destroy",
    ] {
        assert!(src.contains(arm), "missing case arm `{arm}`:\n{src}");
    }
}

#[test]
fn controllers_articles_dispatch_renames_new_action_to_avoid_object_new_shadow() {
    // `def new` would shadow Object#new; spinel renames the action method
    // to `new_action` and the case arm dispatches `:new` to that name.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(src.contains("when :new"), "{src}");
    assert!(src.contains("new_action"), "{src}");
    // The action method itself is `def new_action`, not `def new`.
    assert!(src.contains("def new_action"), "{src}");
    assert!(
        !src.contains("def new\n") && !src.contains("def new ") && !src.contains("def new("),
        "should not emit `def new` — Object#new shadowing risk:\n{src}",
    );
}

#[test]
fn controllers_articles_filter_dispatch_uses_include_check() {
    // `before_action :set_article, only: %i[show edit update destroy]`
    // lowers to `set_article if [:show, :edit, :update, :destroy].include?(action_name)`
    // inside process_action.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    // The conditional is an if-modifier wrapping the call. Don't assert
    // exact whitespace — just that the structural pieces line up.
    assert!(
        src.contains("set_article if") || src.contains("set_article  if"),
        "expected set_article conditional dispatch; got:\n{src}",
    );
    assert!(src.contains(".include?(action_name)"), "{src}");
    for sym in [":show", ":edit", ":update", ":destroy"] {
        assert!(src.contains(sym), "missing filter sym {sym}:\n{src}");
    }
}

#[test]
fn controllers_articles_keeps_filter_target_as_private_method() {
    // set_article and article_params (the private methods after `private`)
    // pass through to the LibraryClass as ordinary methods. They're
    // referenced by process_action's filter dispatch and by action bodies
    // that touch params; keeping them as methods preserves callsites.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(src.contains("def set_article"), "{src}");
    assert!(src.contains("def article_params"), "{src}");
}

#[test]
fn controllers_articles_requires_referenced_models_from_models_dir() {
    // ArticlesController references the `Article` model in action bodies
    // (`Article.includes(...)`, `Article.new`, `Article.find(...)`).
    // Spinel requires it explicitly from `../models/article`.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(src.contains("require_relative \"../models/article\""), "{src}");
}

#[test]
fn controllers_set_article_lowers_params_expect_id_to_indexed_to_i() {
    // `params.expect(:id)` (Rails 8 single-symbol form) lowers to
    // `@params[:id].to_i`. Spinel doesn't have Rails' magic `params`
    // method; request params are a plain Hash on `@params` whose
    // values are strings, so the path :id needs `.to_i` for AR's
    // integer PK.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("Article.find(@params[:id].to_i)"),
        "expected `@params[:id].to_i` lowering; got:\n{src}",
    );
    assert!(
        !src.contains("params.expect(:id)"),
        "params.expect(:id) should be lowered, not preserved:\n{src}",
    );
}

#[test]
fn controllers_article_params_lowers_expect_hash_to_require_permit() {
    // `params.expect(article: [:title, :body])` lowers to
    // `@params.require(:article).permit(:title, :body)` — the
    // strong-params chain spinel's runtime implements.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("@params.require(:article).permit(:title, :body)"),
        "expected require/permit lowering; got:\n{src}",
    );
    assert!(
        !src.contains("params.expect(article:"),
        "params.expect(article: ...) should be lowered:\n{src}",
    );
}

#[test]
fn controllers_polymorphic_redirect_to_ivar_uses_route_helpers_singular_path() {
    // `redirect_to @article, notice: "..."` lowers to
    // `redirect_to(RouteHelpers.article_path(@article.id), notice: "...")`.
    // The .id arg comes from the model's PK; spinel's RouteHelpers
    // expects scalar args, not model instances.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains(
            "redirect_to(RouteHelpers.article_path(@article.id), notice: \"Article was successfully created.\")"
        ),
        "create's redirect_to:\n{src}",
    );
    assert!(
        src.contains(
            "redirect_to(RouteHelpers.article_path(@article.id), notice: \"Article was successfully updated.\", status: :see_other)"
        ),
        "update's redirect_to:\n{src}",
    );
}

#[test]
fn controllers_path_helper_redirect_to_gets_route_helpers_prefix() {
    // `redirect_to articles_path, ...` lowers to
    // `redirect_to(RouteHelpers.articles_path, ...)` — the path helper
    // is a no-recv Send whose method ends in _path; spinel's runtime
    // defines all path helpers as module functions on RouteHelpers.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains(
            "redirect_to(RouteHelpers.articles_path, notice: \"Article was successfully destroyed.\", status: :see_other)"
        ),
        "destroy's redirect_to:\n{src}",
    );
}

#[test]
fn controllers_redirect_to_renders_with_parens_uniformly() {
    // Every redirect_to call site uses parenthesized form so the
    // emitted shape is uniform — the rewriter sets `parenthesized: true`
    // on the outer Send regardless of whether the source had parens.
    // This is what spinel's reference fixture uses.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    let count_total = src.matches("redirect_to").count();
    let count_with_parens = src.matches("redirect_to(").count();
    assert_eq!(
        count_total, count_with_parens,
        "expected every redirect_to with parens; got {count_with_parens}/{count_total} in:\n{src}"
    );
}

#[test]
fn controllers_application_controller_has_no_dispatcher() {
    // ApplicationController has no actions and no filters in real-blog,
    // so process_action shouldn't be synthesized at all — just the
    // empty class declaration.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "application_controller.rb");
    assert!(!src.contains("def process_action"), "{src}");
}

#[test]
fn controllers_index_synthesizes_render_views_call() {
    // Real-blog's `def index; @articles = Article.includes(...)...; end`
    // has no top-level terminal — Rails relies on implicit render. Spinel
    // requires an explicit `render(Views::Articles.index(<ivars>))` call
    // appended to the body.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("render(Views::Articles.index(@articles))"),
        "expected synthesized Views::Articles.index call; got:\n{src}",
    );
}

#[test]
fn controllers_show_views_call_pulls_ivars_from_filter_targets() {
    // `def show; end` has an empty body, but `before_action :set_article`
    // fires for it; @article is set inside set_article. The synthesized
    // render call needs to find ivars across body + filter targets.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("render(Views::Articles.show(@article))"),
        "expected Views::Articles.show(@article); got:\n{src}",
    );
    assert!(
        src.contains("render(Views::Articles.edit(@article))"),
        "expected Views::Articles.edit(@article); got:\n{src}",
    );
}

#[test]
fn controllers_new_action_views_call_uses_action_name_not_method_name() {
    // The view module method is `new` (matches the action symbol), even
    // though the Ruby method is renamed to `new_action` to avoid the
    // Object#new shadow.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("render(Views::Articles.new(@article))"),
        "expected Views::Articles.new(@article); got:\n{src}",
    );
    assert!(
        !src.contains("Views::Articles.new_action"),
        "view-module method should be `new`, not `new_action`:\n{src}",
    );
}

#[test]
fn controllers_render_symbol_in_else_branch_rewrites_to_views_call() {
    // create's `respond_to` block has the HTML-branch
    // `render :new, status: :unprocessable_entity` after unwrap_respond_to.
    // Should rewrite to `render(Views::Articles.new(@article), status: ...)`.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("render(Views::Articles.new(@article), status: :unprocessable_entity)"),
        "expected Views call in create's else branch; got:\n{src}",
    );
    // update has the parallel `render :edit, status: :unprocessable_entity`.
    assert!(
        src.contains("render(Views::Articles.edit(@article), status: :unprocessable_entity)"),
        "expected Views call in update's else branch; got:\n{src}",
    );
}

#[test]
fn controllers_render_symbol_does_not_appear_after_lowering() {
    // No `render :symbol` form should survive the rewrite — every
    // template render lowers to a Views::Module.method(...) call.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    for sym in [":index", ":show", ":new", ":edit"] {
        let render_sym = format!("render {sym}");
        assert!(
            !src.contains(&render_sym),
            "render-symbol form `{render_sym}` should be lowered:\n{src}",
        );
    }
}

#[test]
fn controllers_views_aggregate_required_when_views_referenced() {
    // Once render(Views::Articles.X(...)) appears in a body, the
    // `require_relative "../views"` aggregate header must be present.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("require_relative \"../views\""),
        "expected `../views` require once Views::* is referenced; got:\n{src}",
    );
}

#[test]
fn controllers_no_render_synth_when_action_already_terminates() {
    // destroy ends with `redirect_to articles_path, ...` (terminal); no
    // implicit render should be appended. The body should contain
    // exactly one redirect_to and no Views::Articles.destroy call.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        !src.contains("Views::Articles.destroy"),
        "destroy already redirects; should not synthesize a Views call:\n{src}",
    );
}

#[test]
fn controllers_application_controller_has_no_views_require() {
    // ApplicationController has no actions, no Views references — so no
    // `require_relative "../views"` should appear.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "application_controller.rb");
    assert!(
        !src.contains("require_relative \"../views\""),
        "ApplicationController references no views; should not require:\n{src}",
    );
}

#[test]
fn setter_send_renders_with_space_around_equals() {
    // The lowered initialize/update bodies call setters via
    // `Send { method: "x=", args: [v] }` (since attr_writer methods
    // are named `x=`). emit_send_base rewrites these to
    // `recv.x = v` form.
    let files = lowered_real_blog();
    let src = find(&files, "article.rb");
    assert!(
        src.contains("self.title = attrs[:title]"),
        "expected `self.title = ...` setter form; got:\n{src}",
    );
    assert!(
        !src.contains("self.title= "),
        "setter should not render as fused `x= ` form; got:\n{src}",
    );
}
