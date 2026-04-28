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

fn lowered_real_blog_views() -> Vec<EmittedFile> {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    ruby::emit_lowered_views(&app)
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
        "def initialize(attrs = {})",
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
fn comment_emits_view_require_for_own_partial_and_parent() {
    // Comment references `Views::Comments` (own partial) via the
    // broadcasts_to expansion's `html:` payload. It also references
    // `Views::Articles` via the rewritten parent-cascade in
    // `after_<x>_commit` (Rails-side `article.broadcast_replace_to(...)`
    // → spinel `Broadcasts.replace(stream:, target:, html: Views::Articles.article(parent))`).
    // Both partial requires are present.
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    assert!(src.contains("require_relative \"../views/comments/_comment\""), "{src}");
    assert!(src.contains("require_relative \"../views/articles/_article\""), "{src}");
}

#[test]
fn comment_broadcasts_compose_with_block_form_callback() {
    // Comment has both `broadcasts_to` AND
    // `after_create_commit { article.broadcast_replace_to(...) rescue nil }`.
    // The two sources fold into one method body; broadcasts_to runs
    // first (source order in the lowering), block-form follows.
    // The block-form's Rails-API call is rewritten to spinel-shape.
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    let create_block = src
        .split("def after_create_commit").nth(1).unwrap()
        .split("def ").next().unwrap();
    assert!(
        create_block.contains("Broadcasts.append")
            && create_block.contains("parent = article")
            && create_block.contains("Broadcasts.replace(stream: \"articles\""),
        "expected composed body; got:\n{create_block}",
    );
    // The original broadcasts_to-derived Broadcasts.append appears
    // BEFORE the rewritten parent-cascade — composition order matches
    // source-order semantics.
    let pos_append = create_block.find("Broadcasts.append").unwrap();
    let pos_parent = create_block.find("parent = article").unwrap();
    assert!(pos_append < pos_parent, "{create_block}");
}

#[test]
fn comment_block_callbacks_render_as_methods() {
    // real-blog's Comment has:
    //   after_create_commit { article.broadcast_replace_to("articles") rescue nil }
    //   after_destroy_commit { article.broadcast_replace_to("articles") rescue nil }
    // Lowered to `def after_create_commit; …; end` etc., with the
    // Rails-API broadcast call rewritten to spinel-shape:
    //   parent = article
    //   return if parent.nil?
    //   Broadcasts.replace(stream:, target:, html: Views::Articles.article(parent))
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    assert!(src.contains("def after_create_commit"), "{src}");
    assert!(src.contains("def after_destroy_commit"), "{src}");
    // Rewrite produced the spinel-shape sequence; original `rescue nil`
    // wrapper is dropped (the explicit `return if parent.nil?` covers it).
    assert!(src.contains("parent = article"), "{src}");
    assert!(src.contains("return if parent.nil?"), "{src}");
    assert!(
        src.contains("Broadcasts.replace(stream: \"articles\", target: \"article_#{parent.id}\", html: Views::Articles.article(parent))"),
        "expected spinel-shape parent-cascade; got:\n{src}",
    );
    assert!(
        !src.contains("broadcast_replace_to"),
        "Rails-side broadcast_replace_to should be rewritten away; got:\n{src}",
    );
    assert!(
        !src.contains("rescue nil"),
        "rescue nil should be unwrapped after rewrite; got:\n{src}",
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
fn controllers_index_drops_includes_from_chain() {
    // `Article.includes(:comments).order(...)` — `.includes` is an
    // eager-load optimization with no spinel runtime equivalent. The
    // includes call gets dropped from the chain (correctness-equivalent
    // to plain access without eager loading).
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        !src.contains(".includes("),
        "includes should be dropped from chain:\n{src}",
    );
}

#[test]
fn controllers_index_lowers_order_to_sort_by_with_reverse_for_desc() {
    // `Article.includes(:comments).order(created_at: :desc)` lowers to
    // `Article.all.sort_by { |a| a.created_at.to_s }.reverse`. The
    // bare-Const recv gets `.all` prepended, the kwarg becomes a sort
    // block, and `:desc` direction trails a `.reverse`.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("Article.all"),
        "expected `Article.all` after chain lowering; got:\n{src}",
    );
    assert!(
        src.contains(".sort_by"),
        "expected sort_by; got:\n{src}",
    );
    assert!(
        src.contains("a.created_at.to_s"),
        "expected `a.created_at.to_s` in sort block; got:\n{src}",
    );
    assert!(
        src.contains(".reverse"),
        "expected `.reverse` for :desc direction; got:\n{src}",
    );
    // The original `.order(...)` form must not survive.
    assert!(
        !src.contains(".order(created_at"),
        "order(...) should be lowered to sort_by; got:\n{src}",
    );
}

#[test]
fn controllers_destroy_bang_lowers_to_destroy() {
    // `@article.destroy!` lowers to `@article.destroy`. Spinel's runtime
    // model has only one destroy variant (raise-on-failure semantics);
    // the bang form has no separate behavior to preserve.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(src.contains("@article.destroy"), "{src}");
    assert!(
        !src.contains("@article.destroy!"),
        "destroy! should be lowered to destroy:\n{src}",
    );
}

#[test]
fn controllers_params_helper_calls_get_to_h_at_use_sites() {
    // `Article.new(article_params)` → `Article.new(article_params.to_h)`.
    // Spinel's strong-params chain returns a Parameters-like object;
    // model constructors expect a plain Hash.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("Article.new(article_params.to_h)"),
        "expected `article_params.to_h` in Article.new call; got:\n{src}",
    );
    assert!(
        src.contains("@article.update(article_params.to_h)"),
        "expected `article_params.to_h` in update call; got:\n{src}",
    );
    // The bare form should not appear as a positional arg anywhere in
    // the action bodies.
    assert!(
        !src.contains("Article.new(article_params)"),
        "bare article_params should be wrapped:\n{src}",
    );
    assert!(
        !src.contains(".update(article_params)"),
        "bare article_params should be wrapped:\n{src}",
    );
}

#[test]
fn controllers_params_helper_body_does_not_self_wrap() {
    // The `def article_params` body itself should not get `.to_h` —
    // its body is `@params.require(:article).permit(:title, :body)`
    // with no `<x>_params` Send to rewrite.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    let body = src
        .split("def article_params").nth(1).unwrap()
        .split("end").next().unwrap();
    assert!(
        !body.contains("article_params.to_h"),
        "article_params helper body should not call itself:\n{body}",
    );
    assert!(
        body.contains("@params.require(:article).permit(:title, :body)"),
        "expected unchanged permit chain in helper body:\n{body}",
    );
}

#[test]
fn comments_build_expansion_composes_with_params_to_h_rewrite() {
    // `attrs = comment_params.to_h` should appear exactly once — the
    // build expansion drops `.to_h` from its synthesized form, and the
    // params-to-h rewrite adds it back. Verifying composition order so
    // a regression to `.to_h.to_h` would be caught.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "comments_controller.rb");
    assert!(src.contains("attrs = comment_params.to_h"), "{src}");
    assert!(
        !src.contains("comment_params.to_h.to_h"),
        "double to_h regression — params rewrite should not re-wrap:\n{src}",
    );
}

#[test]
fn comments_create_expands_assoc_build_to_three_statements() {
    // `@comment = @article.comments.build(comment_params)` lowers to
    // three statements: build the attrs hash, set the FK, then call
    // `Comment.new(attrs)`. Mirrors spinel-blog's reference shape.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "comments_controller.rb");
    assert!(
        src.contains("attrs = comment_params.to_h"),
        "expected `attrs = …to_h` first stmt; got:\n{src}",
    );
    assert!(
        src.contains("attrs[:article_id] = @article.id"),
        "expected `attrs[:article_id] = @article.id` second stmt; got:\n{src}",
    );
    assert!(
        src.contains("@comment = Comment.new(attrs)"),
        "expected `@comment = Comment.new(attrs)` third stmt; got:\n{src}",
    );
    // The original `.comments.build(...)` form must not survive.
    assert!(
        !src.contains("@article.comments.build"),
        "assoc.build should be lowered, not preserved:\n{src}",
    );
}

#[test]
fn comments_destroy_expands_assoc_find_to_lookup_plus_belongs_to_guard() {
    // `@comment = @article.comments.find(params.expect(:id))` lowers to
    //   @comment = Comment.find(@params[:id].to_i)
    //   if @comment.article_id != @article.id
    //     head(:not_found)
    //     return
    //   end
    // The guard preserves the belongs-to-article semantics that Rails
    // would have enforced via the through-association lookup.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "comments_controller.rb");
    assert!(
        src.contains("@comment = Comment.find(@params[:id].to_i)"),
        "expected direct Comment.find lookup; got:\n{src}",
    );
    assert!(
        src.contains("if @comment.article_id != @article.id"),
        "expected belongs-to guard predicate; got:\n{src}",
    );
    assert!(
        src.contains("head(:not_found)"),
        "expected head(:not_found) in guard body; got:\n{src}",
    );
    assert!(
        !src.contains("@article.comments.find"),
        "assoc.find should be lowered:\n{src}",
    );
}

#[test]
fn comments_controller_requires_comment_model_after_assoc_lowering() {
    // Once `Comment.new` and `Comment.find` appear in the body, the
    // emitter's body-derived requires should pull in
    // `../models/comment` automatically.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "comments_controller.rb");
    assert!(
        src.contains("require_relative \"../models/comment\""),
        "expected ../models/comment require; got:\n{src}",
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

// ── lowered views ───────────────────────────────────────────────
//
// Drives `emit_lowered_views` against real-blog and asserts the
// articles/index template lowers to the `Views::Articles.index`
// spinel-blog shape: a Views module, an `index` class method, an
// io-buffer body that funnels every static chunk and helper call
// through `io << ...`, and helper-call rewrites
// (`turbo_stream_from`, `content_for`, `link_to`+path-helper,
// `render @collection`).
//
// Structural assertions, not byte-match — surface formatting churn
// shouldn't ripple in. Spinel-blog's hand-written
// `app/views/articles/index.rb` is the visual reference.

#[test]
fn lowered_views_emit_articles_index_file() {
    let files = lowered_real_blog_views();
    let names: Vec<String> = files
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("app/views/articles/index.rb")),
        "{names:?}",
    );
}

#[test]
fn lowered_index_view_renders_module_and_method() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/index.rb");
    assert!(
        src.contains("module Views\n  module Articles"),
        "expected nested `module Views; module Articles`; got:\n{src}",
    );
    assert!(
        src.contains("def self.index("),
        "expected `def self.index(...)`; got:\n{src}",
    );
    assert!(
        src.contains("io = String.new"),
        "expected `io = String.new` prologue; got:\n{src}",
    );
}

#[test]
fn lowered_index_view_rewrites_view_helpers() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/index.rb");
    // `<%= turbo_stream_from "articles" %>` → ViewHelpers call.
    assert!(
        src.contains("ViewHelpers.turbo_stream_from(\"articles\")"),
        "expected ViewHelpers.turbo_stream_from rewrite; got:\n{src}",
    );
    // `<% content_for :title, "Articles" %>` → ViewHelpers setter.
    assert!(
        src.contains("ViewHelpers.content_for_set(:title, \"Articles\")"),
        "expected ViewHelpers.content_for_set rewrite; got:\n{src}",
    );
    // `<%= link_to "New article", new_article_path, class: "..." %>`
    // → ViewHelpers.link_to with path-helper rewrite.
    assert!(
        src.contains("ViewHelpers.link_to"),
        "expected ViewHelpers.link_to rewrite; got:\n{src}",
    );
    assert!(
        src.contains("RouteHelpers.new_article_path"),
        "expected RouteHelpers.new_article_path rewrite; got:\n{src}",
    );
}

#[test]
fn lowered_index_view_renders_collection_partial_via_each() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/index.rb");
    // `<%= render @articles %>` → `articles.each { |a| io <<
    // Views::Articles.article(a) }`.
    assert!(
        src.contains("articles.each"),
        "expected collection `each` iteration; got:\n{src}",
    );
    assert!(
        src.contains("Views::Articles.article("),
        "expected per-element partial dispatch; got:\n{src}",
    );
}

#[test]
fn lowered_index_view_auto_escapes_bare_interpolation() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/index.rb");
    // `<%= notice %>` is a bare interpolation (not a recognized
    // helper), so emission funnels through ViewHelpers.html_escape.
    assert!(
        src.contains("ViewHelpers.html_escape(notice)"),
        "expected html_escape on bare `notice` interpolation; got:\n{src}",
    );
}

#[test]
fn lowered_index_view_rewrites_present_to_negated_empty() {
    // Rails-style `notice.present?` lowers to `! recv.empty?` in spinel
    // shape (collection emptiness rather than the Rails predicate). The
    // unary-not is emitted via `Send { recv: None, method: "!", args:
    // [empty_call] }` so the surface form is `! notice.empty?`, not
    // `notice.empty?.!`.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/index.rb");
    assert!(
        src.contains("! notice.empty?"),
        "expected `! notice.empty?`; got:\n{src}",
    );
    assert!(
        !src.contains("notice.present?"),
        "Rails-style `.present?` should be rewritten away; got:\n{src}",
    );
    assert!(
        !src.contains("notice.empty?.!"),
        "unary-! should not render as method-call form; got:\n{src}",
    );
    // Same rewrite applies to `@articles.any?` (after ivar rewrite to
    // `articles.any?`) — the `<% if @articles.any? %>` branch.
    assert!(
        src.contains("! articles.empty?"),
        "expected `! articles.empty?` rewrite of `@articles.any?`; got:\n{src}",
    );
}

#[test]
fn lowered_article_partial_emits_module_method_signature() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_article.rb");
    assert!(
        src.contains("module Views\n  module Articles"),
        "expected nested `module Views; module Articles`; got:\n{src}",
    );
    // Partial methods take the singular form of the directory.
    assert!(
        src.contains("def self.article(article)"),
        "expected `def self.article(article)`; got:\n{src}",
    );
}

#[test]
fn lowered_article_partial_renders_dom_id_with_and_without_prefix() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_article.rb");
    // `<%= dom_id(article) %>` → 1-arg form.
    assert!(
        src.contains("ViewHelpers.dom_id(article)"),
        "expected 1-arg dom_id; got:\n{src}",
    );
    // `<%= dom_id(article, :comments_count) %>` → 2-arg form preserves
    // the symbol prefix.
    assert!(
        src.contains("ViewHelpers.dom_id(article, :comments_count)"),
        "expected 2-arg dom_id with sym prefix; got:\n{src}",
    );
}

#[test]
fn lowered_article_partial_pluralize_uses_inflector() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_article.rb");
    // `<%= pluralize(article.comments.size, "comment") %>` →
    // Inflector.pluralize (separate from ActiveSupport's string
    // pluralization helpers; spinel-blog convention).
    assert!(
        src.contains("Inflector.pluralize(article.comments.size, \"comment\")"),
        "expected Inflector.pluralize; got:\n{src}",
    );
}

#[test]
fn lowered_article_partial_truncate_wrapped_in_html_escape() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_article.rb");
    // `<%= truncate(article.body, length: 100) %>` returns a plain
    // string in spinel's runtime — so the lowering wraps it in
    // html_escape. link_to / button_to / dom_id stay raw because they
    // already return escape-correct output.
    assert!(
        src.contains("ViewHelpers.html_escape(ViewHelpers.truncate(article.body, length: 100))"),
        "expected html_escape-wrapped truncate; got:\n{src}",
    );
}

#[test]
fn lowered_article_partial_link_to_record_uses_singular_path_helper() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_article.rb");
    // `<%= link_to article.title, article, ... %>` — the URL arg is a
    // bare local record. Lowering rewrites to `RouteHelpers
    // .article_path(article.id)`.
    assert!(
        src.contains("ViewHelpers.link_to(article.title, RouteHelpers.article_path(article.id)"),
        "expected link_to(text, RouteHelpers.article_path(article.id), …); got:\n{src}",
    );
    // `<%= link_to "Show", article, ... %>` — same pattern, literal text.
    assert!(
        src.contains("ViewHelpers.link_to(\"Show\", RouteHelpers.article_path(article.id)"),
        "expected `Show` link to article record; got:\n{src}",
    );
    // `<%= link_to "Edit", edit_article_path(article), ... %>` —
    // path-helper URL with bare-local arg → `article.id`.
    assert!(
        src.contains("ViewHelpers.link_to(\"Edit\", RouteHelpers.edit_article_path(article.id)"),
        "expected `Edit` link with edit_article_path(article.id); got:\n{src}",
    );
}

#[test]
fn lowered_article_partial_button_to_record_with_options() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_article.rb");
    // `<%= button_to "Destroy", article, method: :delete, ... %>` —
    // ButtonTo classifier produces a `RouteHelpers.article_path(.id)`
    // URL and threads the opts hash through unchanged.
    assert!(
        src.contains("ViewHelpers.button_to(\"Destroy\", RouteHelpers.article_path(article.id)"),
        "expected button_to with article_path; got:\n{src}",
    );
    assert!(
        src.contains("method: :delete"),
        "expected `method: :delete` opts entry; got:\n{src}",
    );
}

#[test]
fn lowered_form_partial_emits_module_method() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_form.rb");
    assert!(
        src.contains("module Views\n  module Articles"),
        "expected nested `module Views; module Articles`; got:\n{src}",
    );
    assert!(
        src.contains("def self.form(article)"),
        "expected `def self.form(article)`; got:\n{src}",
    );
}

#[test]
fn lowered_form_partial_form_with_capture_uses_inner_body_accumulator() {
    // `<%= form_with(...) do |form| ... %>` lowers to a
    // `ViewHelpers.form_with(...) do |form| body = String.new ; … ;
    // body end` capture. Inner template stmts append to `body`, not
    // the outer `io`, so the captured string is what the form_with
    // helper consumes.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_form.rb");
    assert!(
        src.contains("io << ViewHelpers.form_with(model: article"),
        "expected outer io append of form_with; got:\n{src}",
    );
    assert!(
        src.contains("do |form|"),
        "expected `do |form|` block; got:\n{src}",
    );
    // Inner accumulator is `body`, not `io`.
    assert!(
        src.contains("body = String.new"),
        "expected fresh `body = String.new` inside the form_with block; got:\n{src}",
    );
    assert!(
        src.contains("body << form.label"),
        "expected `body << form.label(...)` (inner accumulator) inside the block; got:\n{src}",
    );
}

#[test]
fn lowered_form_partial_errors_predicate_rewrite() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_form.rb");
    // `<% if article.errors.any? %>` → `if ! article.errors.empty?`
    // (predicate-cond rewrite applied through the receiver chain).
    assert!(
        src.contains("if ! article.errors.empty?"),
        "expected predicate-rewrite on errors.any?; got:\n{src}",
    );
    assert!(
        !src.contains("article.errors.any?"),
        "Rails-style `.any?` should be rewritten away; got:\n{src}",
    );
}

#[test]
fn lowered_form_partial_form_builder_methods() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_form.rb");
    // `form.label :title` → `form.label(:title)` — pass-through.
    assert!(
        src.contains("body << form.label(:title)"),
        "expected form.label dispatch; got:\n{src}",
    );
    // `form.text_field :title, class: [...]` →
    // `form.text_field(:title, class: "<base-string>")` — class
    // array collapses to its first element.
    assert!(
        src.contains("body << form.text_field(:title, class: \"block shadow-sm rounded-md border px-3 py-2 mt-2 w-full\")"),
        "expected form.text_field with class-array simplified to base string; got:\n{src}",
    );
    // `form.textarea :body, rows: 4, ...` → `form.text_area(:body,
    // rows: 4, ...)` — alias normalized to underscore form.
    assert!(
        src.contains("body << form.text_area(:body, rows: 4"),
        "expected form.textarea aliased to text_area; got:\n{src}",
    );
    assert!(
        !src.contains("form.textarea("),
        "form.textarea alias should not survive; got:\n{src}",
    );
    // `form.submit class: "..."` → `form.submit(nil, class: "...")` —
    // leading `nil` inserted when no positional arg was provided.
    assert!(
        src.contains("body << form.submit(nil, class: "),
        "expected leading-nil insertion on form.submit; got:\n{src}",
    );
}

#[test]
fn lowered_form_partial_errors_each_iterates_with_html_escape() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_form.rb");
    // `<% article.errors.each do |error| %>...<% end %>` walks as a
    // template-level each block, body indented under it and using
    // the active accumulator (`body` since we're inside form_with).
    assert!(
        src.contains("article.errors.each do |error|"),
        "expected article.errors.each block; got:\n{src}",
    );
    // `<%= error.full_message %>` becomes just `error` after the
    // errors-each adapter rewrite (spinel-runtime errors are plain
    // Strings, no `full_message` method). Auto-escape still applies.
    assert!(
        src.contains("body << ViewHelpers.html_escape(error)"),
        "expected html_escape on bare `error` (full_message stripped); got:\n{src}",
    );
    assert!(
        !src.contains("error.full_message"),
        "Rails-side `.full_message` should be rewritten away; got:\n{src}",
    );
}

#[test]
fn lowered_form_partial_pluralize_count_uses_inflector() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_form.rb");
    // `<%= pluralize(article.errors.count, "error") %>` →
    // `Inflector.pluralize(article.errors.count, "error")`. (spinel-
    // blog uses `.length` instead of `.count` — both work in Ruby;
    // size/length/count normalization is a future slice.)
    assert!(
        src.contains("Inflector.pluralize(article.errors.count, \"error\")"),
        "expected Inflector.pluralize on errors.count; got:\n{src}",
    );
}

// ── comments/_comment.html.erb ──────────────────────────────────

#[test]
fn lowered_comment_partial_emits_module_method() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/comments/_comment.rb");
    assert!(
        src.contains("module Views\n  module Comments"),
        "expected nested `module Views; module Comments`; got:\n{src}",
    );
    assert!(
        src.contains("def self.comment(comment)"),
        "expected `def self.comment(comment)`; got:\n{src}",
    );
}

#[test]
fn lowered_comment_partial_nested_url_array_to_path_helper() {
    // `<%= button_to "Delete", [comment.article, comment], method:
    // :delete, ... %>` lowers the nested-resource array to
    // `RouteHelpers.article_comment_path(comment.article_id,
    // comment.id)`. The parent `comment.article` is a belongs_to
    // read; we use the FK column `comment.article_id` (avoiding the
    // dereference) and the child `comment.id` directly.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/comments/_comment.rb");
    assert!(
        src.contains(
            "ViewHelpers.button_to(\"Delete\", RouteHelpers.article_comment_path(comment.article_id, comment.id)"
        ),
        "expected nested-array URL → article_comment_path with FK + id; got:\n{src}",
    );
}

#[test]
fn lowered_comment_partial_auto_escape_on_attrs() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/comments/_comment.rb");
    // Bare-attr interpolations get html_escape on the way to io.
    assert!(
        src.contains("ViewHelpers.html_escape(comment.commenter)"),
        "expected html_escape(comment.commenter); got:\n{src}",
    );
    assert!(
        src.contains("ViewHelpers.html_escape(comment.body)"),
        "expected html_escape(comment.body); got:\n{src}",
    );
}

// ── articles/show.html.erb ──────────────────────────────────────

#[test]
fn lowered_show_view_emits_module_method() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/show.rb");
    assert!(
        src.contains("module Views\n  module Articles"),
        "expected nested `module Views; module Articles`; got:\n{src}",
    );
    assert!(
        src.contains("def self.show(article"),
        "expected `def self.show(article, ...)`; got:\n{src}",
    );
}

#[test]
fn lowered_show_view_renders_association_partial_via_each() {
    // `<%= render @article.comments %>` — has_many association
    // partial. Lowered to `article.comments.each { |c| io <<
    // Views::Comments.comment(c) }`. The receiver is the
    // post-ivar-rewrite `article`; the var name is the singular's
    // first letter (`c`).
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/show.rb");
    assert!(
        src.contains("article.comments.each { |c| io << Views::Comments.comment(c) }"),
        "expected article.comments.each association iteration; got:\n{src}",
    );
}

#[test]
fn lowered_show_view_turbo_stream_from_with_string_interp() {
    // `<%= turbo_stream_from "article_#{@article.id}_comments" %>`
    // — the channel is a StringInterp with an ivar that rewrites
    // through to `"article_#{article.id}_comments"`. Helper emits
    // raw (no html_escape — turbo_stream_from is html-safe).
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/show.rb");
    assert!(
        src.contains("ViewHelpers.turbo_stream_from(\"article_#{article.id}_comments\")"),
        "expected turbo_stream_from with interpolated channel; got:\n{src}",
    );
}

#[test]
fn lowered_show_view_form_with_nested_array_model_dispatches_form_builder() {
    // `<%= form_with model: [@article, Comment.new], ... do |form|
    // %>` — polymorphic-array form_with for a nested resource.
    // The lowerer rewrites it to spinel's expected shape:
    // `model: Comment.new` (the child), `model_name: "comment"`,
    // `action: RouteHelpers.article_comments_path(article.id)` (the
    // nested collection path), `method: :post` (Class.new is never
    // persisted). FormBuilder dispatch resolves inside the block.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/show.rb");
    assert!(
        src.contains("ViewHelpers.form_with(model: Comment.new"),
        "expected form_with with nested-array model rewritten to child; got:\n{src}",
    );
    assert!(
        src.contains("model_name: \"comment\""),
        "expected model_name derived from child class; got:\n{src}",
    );
    assert!(
        src.contains("action: RouteHelpers.article_comments_path(article.id)"),
        "expected nested collection path action; got:\n{src}",
    );
    assert!(
        src.contains("method: :post"),
        "expected method :post (Class.new is never persisted); got:\n{src}",
    );
    // FormBuilder dispatch — direct sends, not wrapped in html_escape.
    assert!(
        src.contains("body << form.label(:commenter, class: \"block font-medium\")"),
        "expected form.label dispatch; got:\n{src}",
    );
    assert!(
        src.contains("body << form.text_field(:commenter, class: \"block w-full border rounded p-2\")"),
        "expected form.text_field dispatch; got:\n{src}",
    );
    assert!(
        src.contains("body << form.text_area(:body, rows: 3"),
        "expected form.text_area dispatch (with textarea→text_area alias); got:\n{src}",
    );
    // `form.submit "Add Comment", class: "..."` — a positional
    // String already, so no leading-nil insertion (unlike
    // `form.submit class: "..."` in articles/_form.rb).
    assert!(
        src.contains("body << form.submit(\"Add Comment\", class: "),
        "expected form.submit with positional label preserved; got:\n{src}",
    );
    assert!(
        !src.contains("ViewHelpers.html_escape(form."),
        "FormBuilder calls should not be html_escape-wrapped; got:\n{src}",
    );
}

// ── layouts/application.html.erb ────────────────────────────────

#[test]
fn lowered_layout_view_emits_module_method() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/layouts/application.rb");
    assert!(
        src.contains("module Views\n  module Layouts"),
        "expected nested `module Views; module Layouts`; got:\n{src}",
    );
    // Layouts take an explicit `body` parameter — bare `<%= yield %>`
    // in the source resolves to this local.
    assert!(
        src.contains("def self.application(body)"),
        "expected `def self.application(body)`; got:\n{src}",
    );
}

#[test]
fn lowered_layout_view_bare_yield_renders_body_local() {
    // `<%= yield %>` (no slot arg) is the layout's body slot — lowers
    // to `io << body` (the explicit param), not a ViewHelpers call.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/layouts/application.rb");
    assert!(
        src.contains("io << body"),
        "expected `io << body` from bare yield; got:\n{src}",
    );
}

#[test]
fn lowered_layout_view_yield_slot_uses_get_slot() {
    // `<%= yield :head %>` → `ViewHelpers.get_slot(:head)`.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/layouts/application.rb");
    assert!(
        src.contains("io << ViewHelpers.get_slot(:head)"),
        "expected `ViewHelpers.get_slot(:head)` for yielded slot; got:\n{src}",
    );
}

#[test]
fn lowered_layout_view_head_helpers() {
    // The bare zero-arg layout helpers all dispatch to ViewHelpers.*.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/layouts/application.rb");
    assert!(
        src.contains("io << ViewHelpers.csrf_meta_tags"),
        "expected csrf_meta_tags rewrite; got:\n{src}",
    );
    assert!(
        src.contains("io << ViewHelpers.csp_meta_tag"),
        "expected csp_meta_tag rewrite; got:\n{src}",
    );
    assert!(
        src.contains("io << ViewHelpers.javascript_importmap_tags"),
        "expected javascript_importmap_tags rewrite; got:\n{src}",
    );
}

#[test]
fn lowered_layout_view_stylesheet_link_tag_expands_app_sym() {
    // `<%= stylesheet_link_tag :app, "data-turbo-track": "reload" %>`
    // expands to one call per ingested stylesheet (real-blog has
    // `application` from app/assets/stylesheets/ + `tailwind` from
    // app/assets/builds/), joined by "\n    " so they render as two
    // adjacent <link> tags. Mirrors Rails' Propshaft `:app` resolution
    // and the per-target view emitters' :app expansion.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/layouts/application.rb");
    assert!(
        src.contains("ViewHelpers.stylesheet_link_tag(\"application\""),
        "expected `application` stylesheet link; got:\n{src}",
    );
    assert!(
        src.contains("ViewHelpers.stylesheet_link_tag(\"tailwind\""),
        "expected `tailwind` stylesheet link; got:\n{src}",
    );
    assert!(
        src.contains("\"data-turbo-track\""),
        "expected `data-turbo-track` opts entry preserved; got:\n{src}",
    );
    assert!(
        !src.contains("stylesheet_link_tag(\"app\""),
        "the literal `\"app\"` arg should have been expanded away; got:\n{src}",
    );
}

#[test]
fn lowered_layout_view_content_for_default_via_bool_op() {
    // `<%= content_for(:title) || "Real Blog" %>` — the BoolOp's
    // left side is a bare `content_for(:title)` Send. The auto-
    // escape path's `rewrite_helpers_in_expr` recurses through the
    // BoolOp and rewrites the inner helper to its ViewHelpers
    // form before the html_escape wrap is added.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/layouts/application.rb");
    assert!(
        src.contains(
            "ViewHelpers.html_escape(ViewHelpers.content_for_get(:title) || \"Real Blog\")"
        ),
        "expected nested helper rewrite under BoolOp + outer html_escape; got:\n{src}",
    );
    // Make sure the raw `content_for(:title)` Send did not survive.
    assert!(
        !src.contains("content_for(:title) ||"),
        "raw content_for Send should be rewritten; got:\n{src}",
    );
}

// ── articles/new + articles/edit (named-partial dispatch) ───────

#[test]
fn lowered_new_view_dispatches_named_partial() {
    // `<%= render "form", article: @article %>` is the Named
    // RenderPartial variant. Bare partial name `"form"` routes to
    // the current resource_dir's module — `Views::Articles.form
    // (article)`. The hash's first value (`@article` post-ivar
    // rewrite → `article`) becomes the call's positional arg.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/new.rb");
    assert!(
        src.contains("def self.new(article)"),
        "expected `def self.new(article)`; got:\n{src}",
    );
    assert!(
        src.contains("io << Views::Articles.form(article)"),
        "expected named-partial dispatch to Views::Articles.form(article); got:\n{src}",
    );
}

#[test]
fn lowered_edit_view_dispatches_named_partial_and_record_link() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/edit.rb");
    assert!(
        src.contains("def self.edit(article)"),
        "expected `def self.edit(article)`; got:\n{src}",
    );
    // Same shared partial as `new.html.erb`.
    assert!(
        src.contains("io << Views::Articles.form(article)"),
        "expected named-partial dispatch; got:\n{src}",
    );
    // `<%= link_to "Show this article", @article, ... %>` — the URL
    // arg is the bare local record (post-ivar-rewrite), so it
    // resolves to `RouteHelpers.article_path(article.id)`.
    assert!(
        src.contains("ViewHelpers.link_to(\"Show this article\", RouteHelpers.article_path(article.id)"),
        "expected link_to(text, RouteHelpers.article_path(article.id)) for record-ref URL; got:\n{src}",
    );
}
