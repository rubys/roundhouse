//! Regression test for the lower → Ruby emit pipeline. Drives
//! `emit_lowered_models` against `fixtures/real-blog` and asserts the
//! emitted source matches the universal post-lowering shape. The test
//! asserts structural equivalents (key methods present with the right
//! body shapes) rather than byte-for-byte match, so surface-formatting
//! churn doesn't ripple in.
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
        // `update` now takes the typed `<Resource>Params` (`p`) when a
        // controller permits the model. Hash-shaped `update(attrs)`
        // remains for models without a permit declaration.
        "def update(p)",
    ] {
        assert!(src.contains(m), "missing `{m}`:\n{src}");
    }
}

#[test]
fn article_renders_validate_with_inline_checks() {
    let files = lowered_real_blog();
    let src = find(&files, "article.rb");
    assert!(src.contains("def validate"), "{src}");
    // Presence rules expand to inline IR (Phase 2.5(a) lowerer per
    // docs/rust-migration-plan.md) — no helper-call into the
    // Validations runtime module. Error messages become string-literal
    // constants; the type-erased `value: untyped` channel is gone.
    // The Ruby emitter uses postfix-modifier form for single-line
    // if's, so the assertion matches the natural output shape.
    assert!(
        src.contains(r#"errors << "Title can't be blank" if @title.nil?"#),
        "{src}",
    );
    assert!(
        src.contains(r#"errors << "Body can't be blank" if @body.nil?"#),
        "{src}",
    );
    // Length is now also inline (third slice of Phase 2.5(a)).
    // Expansion: `unless @body.nil?; len = ...; errors << "..." if len < N; end`.
    // Message matches Rails' full_messages: humanized attribute +
    // "characters" suffix (see model_to_library::validations::humanize).
    assert!(
        src.contains(
            r#"errors << "Body is too short (minimum is 10 characters)" if len < 10"#
        ),
        "{src}",
    );
}

#[test]
fn article_renders_residualized_fill_timestamps() {
    let files = lowered_real_blog();
    let src = find(&files, "article.rb");
    // `ActiveRecord::Base#fill_timestamps` probes the schema at runtime
    // (`schema_columns.include?(:updated_at)`) on every save. Column
    // presence is compile-time-constant, so the per-model override drops
    // the `include?` guards and emits only the live assignments —
    // `updated_at` on every save, `created_at` only on insert. The Ruby
    // emitter uses postfix-modifier form for the single-line `if`.
    assert!(src.contains("def fill_timestamps(creating)"), "{src}");
    // `ActiveSupport.db_now` — Rails' exact storage form
    // ("YYYY-MM-DD HH:MM:SS.ffffff" UTC), not `Time.now.utc.iso8601`:
    // stamps must byte-match what Rails writes so TEXT ordering stays
    // correct in a shared database.
    assert!(src.contains("now = ActiveSupport.db_now"), "{src}");
    // Timestamps are temporal columns — stamps land on the `<col>_raw`
    // storage ivar (the public reader parses it to Time).
    assert!(src.contains("@updated_at_raw = now"), "{src}");
    assert!(src.contains("@created_at_raw = now if creating"), "{src}");
    // The runtime schema probe must be fully residualized away.
    assert!(
        !src.contains(".include?(:updated_at)") && !src.contains(".include?(:created_at)"),
        "fill_timestamps still probes the schema at runtime:\n{src}",
    );
}

#[test]
fn article_renders_has_many_reader_and_dependent_destroy() {
    // The has_many proxy body started as `Comment.where(article_id:
    // @id)` and is rewritten by the Arel pass into an inline
    // SELECT/hydrate over the Db primitive surface — the proxy now
    // returns hydrated Comment instances directly without going
    // through framework Ruby's Base#where (which Level-3 retired).
    // See project_arel_compile_time_first.md.
    let files = lowered_real_blog();
    let src = find(&files, "article.rb");
    assert!(src.contains("def comments"), "{src}");
    assert!(
        src.contains("Db.prepare(") && src.contains("FROM comments"),
        "expected Arel-emitted SELECT in `def comments`; got:\n{src}",
    );
    assert!(
        src.contains("Db.escape_int(@id)"),
        "expected runtime FK value to be escaped via Db.escape_int; got:\n{src}",
    );
    // The per-row hydrate now delegates to the synthesized positional
    // factory `Comment.from_stmt(stmt)` (the column reads live once in
    // Comment's `from_stmt` body, not inlined at every query site).
    assert!(
        src.contains("Comment.from_stmt(stmt)"),
        "expected hydrate loop to call Comment.from_stmt(stmt); got:\n{src}",
    );
    assert!(
        src.contains("def before_destroy") && src.contains("comments.each"),
        "{src}",
    );
}

#[test]
fn comment_renders_belongs_to_with_fk_guard() {
    // The belongs_to body started as `Article.find_by(id:
    // @article_id)` and is rewritten by the Arel pass into a
    // single-row SELECT/hydrate (LIMIT 1, nilable result). The fk
    // guard `if @article_id == 0` still sits around the lookup.
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    assert!(src.contains("class Comment < ApplicationRecord"), "{src}");
    assert!(src.contains("def article"), "{src}");
    assert!(
        src.contains("Db.prepare(") && src.contains("FROM articles"),
        "expected Arel-emitted SELECT in `def article`; got:\n{src}",
    );
    assert!(
        src.contains("LIMIT 1"),
        "find_by → Arel Select with LIMIT 1; got:\n{src}",
    );
    assert!(
        src.contains("Db.escape_int(@article_id)"),
        "expected runtime FK value to be escaped via Db.escape_int; got:\n{src}",
    );
    // Single-row hydrate delegates to the positional factory
    // `Article.from_stmt(stmt)` (see synth_from_stmt).
    assert!(
        src.contains("Article.from_stmt(stmt)"),
        "expected hydrate to call Article.from_stmt(stmt); got:\n{src}",
    );
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
    //   - Views (per-app aggregator that loads every view module)
    // Sibling `Comment` is autoloaded; no require for it.
    // The aggregator pattern (vs per-template requires) lets any
    // `Views::X.method` call resolve regardless of which template
    // file the method lives in — same as spinel-blog's hand-written
    // `app/views.rb`.
    let files = lowered_real_blog();
    let src = find(&files, "article.rb");
    assert!(src.contains("require_relative \"application_record\""), "{src}");
    assert!(src.contains("require_relative \"../../runtime/broadcasts\""), "{src}");
    assert!(src.contains("require_relative \"../views\""), "{src}");
    assert!(!src.contains("require_relative \"comment\""), "{src}");
}

#[test]
fn comment_emits_view_require_for_own_partial_and_parent() {
    // Comment references `Views::Comments` (own partial) via the
    // broadcasts_to expansion's `html:` payload. It also references
    // `Views::Articles` via the rewritten parent-cascade in
    // `after_<x>_commit` (Rails-side `article.broadcast_replace_to(...)`
    // → spinel `Broadcasts.replace(stream:, target:, html: Views::Articles.article(parent))`).
    // Both resolve through the per-app aggregator at `app/views.rb`.
    let files = lowered_real_blog();
    let src = find(&files, "comment.rb");
    assert!(src.contains("require_relative \"../views\""), "{src}");
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
    // New shape: `def self.statements` returning the DDL list.
    // Reconciled with TS's LibraryFunction shape so the lowerer
    // produces structured data; consumers iterate per-statement.
    assert!(f.content.contains("def self.statements"), "{}", f.content);
}

#[test]
fn schema_emits_create_table_per_table() {
    let src = lowered_real_blog_schema();
    // Each table appears as a CREATE TABLE IF NOT EXISTS string in
    // the statements array. Idempotent guard so re-opening an
    // existing DB is a no-op.
    assert!(src.contains("CREATE TABLE IF NOT EXISTS articles ("), "{src}");
    assert!(src.contains("CREATE TABLE IF NOT EXISTS comments ("), "{src}");
    let table_count = src.matches("CREATE TABLE IF NOT EXISTS").count();
    assert_eq!(table_count, 2, "expected one CREATE TABLE per table; got:\n{src}");
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
    // New shape: indexes appear as bare strings in the statements
    // array (no trailing comma — Ruby array element formatting is
    // emitter-driven now, not heredoc-line-driven).
    assert!(
        src.contains(
            "\"CREATE INDEX IF NOT EXISTS index_comments_on_article_id ON comments (article_id)\""
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
    // New shape: `def self.table` returning the dispatch list +
    // `def self.root` returning the shorthand entry. Reconciled
    // with TS's LibraryFunction shape so the lowerer produces
    // structured data via methods; consumers iterate.
    assert!(f.content.contains("def self.table"), "{}", f.content);
    assert!(f.content.contains("def self.root"), "{}", f.content);
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
    // show, edit, update, destroy). Route rows are typed `Route.new(...)`
    // positional constructor calls — `Route.new(verb, pattern,
    // controller, action)`.
    let src = lowered_real_blog_routes();
    for line in [
        r#"ActionDispatch::Router::Route.new("GET", "/articles", :articles, :index)"#,
        r#"ActionDispatch::Router::Route.new("GET", "/articles/new", :articles, :new)"#,
        r#"ActionDispatch::Router::Route.new("POST", "/articles", :articles, :create)"#,
        r#"ActionDispatch::Router::Route.new("GET", "/articles/:id", :articles, :show)"#,
        r#"ActionDispatch::Router::Route.new("GET", "/articles/:id/edit", :articles, :edit)"#,
        r#"ActionDispatch::Router::Route.new("PATCH", "/articles/:id", :articles, :update)"#,
        r#"ActionDispatch::Router::Route.new("DELETE", "/articles/:id", :articles, :destroy)"#,
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
        src.contains(r#"ActionDispatch::Router::Route.new("POST", "/articles/:article_id/comments", :comments, :create)"#),
        "{src}",
    );
    assert!(
        src.contains(r#"ActionDispatch::Router::Route.new("DELETE", "/articles/:article_id/comments/:id", :comments, :destroy)"#),
        "{src}",
    );
    // Filtered actions must not appear.
    assert!(
        !src.contains(r#":comments, :index"#),
        "only:[:create, :destroy] should drop :index; got:\n{src}",
    );
    assert!(
        !src.contains(r#":comments, :show"#),
        "{src}",
    );
}

#[test]
fn routes_extract_root_into_separate_method() {
    // `root "articles#index"` becomes a top-level `Routes.root`
    // method, not a `Routes.table` entry — the spinel router
    // checks root separately so the dispatch loop doesn't have to
    // special-case "/". (Reconciled to method form 2026-05-02; was
    // a `ROOT = ...freeze` constant before.)
    let src = lowered_real_blog_routes();
    assert!(
        src.contains(
            r#"ActionDispatch::Router::Route.new("GET", "/", :articles, :index)"#
        ),
        "{src}",
    );
    assert!(src.contains("def self.root"), "{src}");
    // root must NOT also be in `table` — extracting it is the whole
    // point.
    let table_section = src.split("def self.table").nth(1).unwrap()
        .split("def self.root").next().unwrap();
    assert!(
        !table_section.contains("\"/\","),
        "root should be hoisted out of table; got table:\n{table_section}",
    );
}

#[test]
fn routes_order_literal_segments_before_id_patterns() {
    // Matching semantics: `/articles/new` must appear before
    // `/articles/:id` so the literal-segment match wins. flatten_routes
    // already orders this way (standard_resource_actions has new before
    // show); regression test against future reordering.
    let src = lowered_real_blog_routes();
    let pos_new = src.find(r#""GET", "/articles/new""#).expect("/articles/new missing");
    let pos_show = src.find(r#""GET", "/articles/:id", :articles, :show"#)
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
fn controllers_articles_inline_set_article_into_filtered_actions() {
    // `before_action :set_article, only: %i[show edit update destroy]`
    // — instead of a filter-dispatch in process_action, the set_article
    // body is inlined at the top of every action that fires it (ticket 8).
    // Self-describing IR: the assignment is materialized at every call
    // site, the body-typer's Seq walk picks it up.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    // process_action no longer contains the filter dispatch.
    assert!(
        !src.contains("set_article if"),
        "set_article filter dispatch should be removed from process_action:\n{src}",
    );
    // set_article body (`@article = Article.find(...)`) is inlined at
    // the top of show, edit, update, destroy. Spot-check on show.
    assert!(
        src.contains("@article = Article.find"),
        "expected inlined @article assignment from set_article:\n{src}",
    );
}

#[test]
fn controllers_articles_drops_pure_filter_targets() {
    // set_article was solely a before_action target — after inlining
    // (ticket 8), the method itself is dead and dropped from emit.
    // article_params is still emitted because action bodies call it
    // directly (it's not a before_action target).
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        !src.contains("def set_article"),
        "set_article should be dropped after inlining; got:\n{src}",
    );
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
    // `@params.fetch("id", "0").to_s.to_i`. @params is
    // `Hash[String, Roundhouse::ParamValue]` (recursive `String |
    // Hash | Array`); `.fetch` returns the union value type, so
    // `.to_s` bridges it to a String leaf before `.to_i`. For the
    // path-param shape this rewrite covers, the value is always a
    // String at runtime — `.to_s` is a no-op (Ruby/Crystal) or
    // `String(x)` coercion (TS). Missing-id sentinel `"0".to_i == 0`.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("Article.find(@params.fetch(\"id\", \"0\").to_s.to_i)"),
        "expected `@params.fetch(\"id\", \"0\").to_s.to_i` lowering; got:\n{src}",
    );
    assert!(
        !src.contains("params.expect(:id)"),
        "params.expect(:id) should be lowered, not preserved:\n{src}",
    );
}

#[test]
fn controllers_article_params_lowers_to_typed_factory() {
    // `params.expect(article: [:title, :body])` and the older
    // `params.require(:article).permit(:title, :body)` both lower to a
    // typed-factory call:
    //   `ArticleParams.from_raw(@params)`
    // The synthesized `ArticleParams` LibraryClass holds the permitted
    // fields as typed slots; `from_raw` dives into the nested resource
    // hash itself (`sub = params.fetch("article", {})`), so the call
    // site passes `@params` raw — no `.require.to_h` chain.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("ArticleParams.from_raw(@params)"),
        "expected typed-factory lowering; got:\n{src}",
    );
    assert!(
        !src.contains("params.expect(article:"),
        "params.expect(article: ...) should be lowered:\n{src}",
    );
    assert!(
        !src.contains(".permit"),
        "permit chain should be replaced by from_raw:\n{src}",
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
        src.contains("render(Views::Articles.index(@articles, @flash[:notice], @flash[:alert]))"),
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
        src.contains("render(Views::Articles.show(@article, @flash[:notice], @flash[:alert]))"),
        "expected Views::Articles.show(@article, ...flash...); got:\n{src}",
    );
    assert!(
        src.contains("render(Views::Articles.edit(@article, @flash[:notice], @flash[:alert]))"),
        "expected Views::Articles.edit(@article, ...flash...); got:\n{src}",
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
        src.contains("render(Views::Articles.new(@article, @flash[:notice], @flash[:alert]))"),
        "expected Views::Articles.new(@article, ...flash...); got:\n{src}",
    );
    assert!(
        !src.contains("Views::Articles.new_action"),
        "view-module method should be `new`, not `new_action`:\n{src}",
    );
}

#[test]
fn controllers_render_symbol_in_else_branch_rewrites_to_views_call() {
    // create's `respond_to` block has the HTML-branch
    // `render :new, status: :unprocessable_*` after unwrap_respond_to.
    // Should rewrite to `render(Views::Articles.new(@article), status: ...)`.
    // Rails 8.1.x scaffold renamed `:unprocessable_entity` →
    // `:unprocessable_content` mid-version; accept either since the
    // fixture's exact keyword depends on which point release of Rails
    // generate-fixture's `gem install rails` resolved.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    let create_entity =
        "render(Views::Articles.new(@article, @flash[:notice], @flash[:alert]), status: :unprocessable_entity)";
    let create_content =
        "render(Views::Articles.new(@article, @flash[:notice], @flash[:alert]), status: :unprocessable_content)";
    assert!(
        src.contains(create_entity) || src.contains(create_content),
        "expected Views call in create's else branch; got:\n{src}",
    );
    // update has the parallel `render :edit, status: :unprocessable_*`.
    let update_entity =
        "render(Views::Articles.edit(@article, @flash[:notice], @flash[:alert]), status: :unprocessable_entity)";
    let update_content =
        "render(Views::Articles.edit(@article, @flash[:notice], @flash[:alert]), status: :unprocessable_content)";
    assert!(
        src.contains(update_entity) || src.contains(update_content),
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
fn controllers_index_lowers_chain_to_inline_select_with_order_by() {
    // `Article.includes(:comments).order(created_at: :desc)` lifts
    // through the Arel pass into an inline SELECT/hydrate over the
    // Db primitive surface. `.includes` drops as a no-op chain link
    // (eager-loading deferred to Phase 3+); `.order(created_at:
    // :desc)` becomes a SQL `ORDER BY created_at DESC` clause.
    // Replaces the previous in-memory `Article.all.sort_by{…}.reverse`
    // shape now that real-blog routes through Arel. See
    // project_arel_compile_time_first.md.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("Db.prepare(") && src.contains("FROM articles"),
        "expected Arel-emitted SELECT in `def index`; got:\n{src}",
    );
    assert!(
        src.contains("ORDER BY created_at DESC"),
        "expected SQL ORDER BY clause; got:\n{src}",
    );
    // Per-row hydrate delegates to the positional factory
    // `Article.from_stmt(stmt)` (see synth_from_stmt).
    assert!(
        src.contains("Article.from_stmt(stmt)"),
        "expected per-row hydrate loop to call Article.from_stmt(stmt); got:\n{src}",
    );
    // Surface forms that the legacy chain rewrites used to produce
    // — these must NOT survive Arel's lift.
    assert!(
        !src.contains(".sort_by"),
        "Arel emit should not fall back to in-memory sort_by; got:\n{src}",
    );
    assert!(
        !src.contains(".order(created_at"),
        "raw .order(...) should be lifted, not surface in emit; got:\n{src}",
    );
    assert!(
        !src.contains(".includes("),
        "raw .includes(...) should be dropped by Arel chain pass; got:\n{src}",
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
fn controllers_params_helper_use_sites_call_typed_factory() {
    // `Article.new(article_params)` → `Article.from_params(article_params)`.
    // `article_params` returns the typed `<Resource>Params` object;
    // the model's `from_params` factory takes that typed value and
    // assigns each permitted field through the column setter. The
    // legacy `.to_h`-wrap-then-Hash-receive shape is gone.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    assert!(
        src.contains("Article.from_params(article_params)"),
        "expected `Article.from_params(article_params)`; got:\n{src}",
    );
    assert!(
        src.contains("@article.update(article_params)"),
        "expected `update(article_params)` (typed); got:\n{src}",
    );
    // Legacy forms must not appear.
    assert!(
        !src.contains("article_params.to_h"),
        "no `.to_h` wrap in typed-factory shape:\n{src}",
    );
    assert!(
        !src.contains("Article.new(article_params"),
        "Article.new(article_params...) should be rewritten to from_params:\n{src}",
    );
}

#[test]
fn controllers_params_helper_body_is_from_raw_call() {
    // The `def article_params` body lowers to a single
    // `ArticleParams.from_raw(@params)` call — the boundary where
    // `Hash[String, untyped]` widens once into typed slots. from_raw
    // dives into the nested resource hash itself; no `.require.to_h`
    // chain, no `permit` chain.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "articles_controller.rb");
    let body = src
        .split("def article_params").nth(1).unwrap()
        .split("end").next().unwrap();
    assert!(
        body.contains("ArticleParams.from_raw(@params)"),
        "expected typed-factory body; got:\n{body}",
    );
    assert!(
        !body.contains(".permit"),
        "permit chain should be replaced by from_raw call:\n{body}",
    );
    assert!(
        !body.contains("article_params.to_h"),
        "article_params body should not self-wrap:\n{body}",
    );
}

#[test]
fn comments_build_expansion_uses_typed_factory() {
    // The build expansion produces typed-factory shape: a single
    // `Comment.from_params(comment_params)` call followed by the FK
    // setter. No intermediate Hash-shaped `attrs` variable; the
    // legacy `attrs = ...; attrs[:fk] = ...; Comment.new(attrs)`
    // 3-statement form is replaced.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "comments_controller.rb");
    assert!(
        !src.contains("attrs = comment_params"),
        "no Hash-shaped attrs intermediate:\n{src}",
    );
    assert!(
        !src.contains("attrs[:article_id]"),
        "no attrs index assignment:\n{src}",
    );
    assert!(
        !src.contains("Comment.new(attrs)"),
        "no Comment.new(attrs) — should be Comment.from_params(...):\n{src}",
    );
}

#[test]
fn comments_create_expands_assoc_build_to_typed_factory_with_fk() {
    // `@comment = @article.comments.build(comment_params)` lowers to:
    //   @comment = Comment.from_params(comment_params)
    //   @comment.article_id = @article.id
    // The typed factory absorbs the permitted-fields assignment; the
    // FK setter follows the model's typed `attr_writer`.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "comments_controller.rb");
    assert!(
        src.contains("@comment = Comment.from_params(comment_params)"),
        "expected typed-factory in build; got:\n{src}",
    );
    assert!(
        src.contains("@comment.article_id = @article.id"),
        "expected FK setter after typed factory; got:\n{src}",
    );
    assert!(
        !src.contains("@article.comments.build"),
        "assoc.build should be lowered, not preserved:\n{src}",
    );
}

#[test]
fn comments_destroy_expands_assoc_find_to_lookup_plus_belongs_to_guard() {
    // `@comment = @article.comments.find(params.expect(:id))` lowers to
    //   @comment = Comment.find(@params.fetch("id", "0").to_s.to_i)
    //   if @comment.article_id != @article.id
    //     head(:not_found)
    //     return
    //   end
    // The guard preserves the belongs-to-article semantics that Rails
    // would have enforced via the through-association lookup. The
    // leading `.to_s` bridges the recursive `Roundhouse::ParamValue`
    // union to a String leaf before `.to_i` — see
    // `controllers_set_article_lowers_params_expect_id_to_indexed_to_i`.
    let files = lowered_real_blog_controllers();
    let src = find(&files, "comments_controller.rb");
    assert!(
        src.contains("@comment = Comment.find(@params.fetch(\"id\", \"0\").to_s.to_i)"),
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
        src.contains("ActionView::ViewHelpers.turbo_stream_from(\"articles\")"),
        "expected ViewHelpers.turbo_stream_from rewrite; got:\n{src}",
    );
    // `<% content_for :title, "Articles" %>` → ViewHelpers setter.
    assert!(
        src.contains("ActionView::ViewHelpers.content_for_set(:title, \"Articles\")"),
        "expected ViewHelpers.content_for_set rewrite; got:\n{src}",
    );
    // `<%= link_to "New article", new_article_path, class: "..." %>`
    // → inline `<a href="<escaped>" class="...">New article</a>`
    // (Stage 2 of the macro-inline retirement; no runtime
    // ViewHelpers.link_to call survives).
    assert!(
        !src.contains("ViewHelpers.link_to"),
        "ViewHelpers.link_to runtime call should be retired by inline expansion; got:\n{src}",
    );
    assert!(
        src.contains("<a href=\\\""),
        "expected inline <a href> tag; got:\n{src}",
    );
    assert!(
        src.contains("RouteHelpers.new_article_path"),
        "expected RouteHelpers.new_article_path rewrite; got:\n{src}",
    );
    // html_escape of the literal label folds at lower time → plain
    // interpolation of the (special-char-free) string.
    assert!(
        src.contains("#{\"New article\"}") && src.contains("</a>"),
        "expected constant-folded link text + closing </a>; got:\n{src}",
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
    // The interpolated value is `.to_s`-coerced first — html_escape is
    // monomorphic `(String) -> String`, so non-String/nil interpolations
    // would otherwise crash (`nil.to_s == ""` matches Rails' empty render).
    assert!(
        src.contains("ActionView::ViewHelpers.html_escape(notice.to_s)"),
        "expected html_escape on `.to_s`-coerced bare `notice` interpolation; got:\n{src}",
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
        src.contains("def self.article(article, notice = nil, alert = nil)"),
        "expected `def self.article(article, notice, alert)`; got:\n{src}",
    );
}

#[test]
fn lowered_article_partial_renders_dom_id_with_and_without_prefix() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_article.rb");
    // `<%= dom_id(article) %>` → 1-arg form.
    assert!(
        src.contains("ActionView::ViewHelpers.dom_id(article)"),
        "expected 1-arg dom_id; got:\n{src}",
    );
    // `<%= dom_id(article, :comments_count) %>` → 2-arg form preserves
    // the symbol prefix.
    assert!(
        src.contains("ActionView::ViewHelpers.dom_id(article, :comments_count)"),
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
        src.contains("ActionView::ViewHelpers.html_escape(ActionView::ViewHelpers.truncate(article.body, length: 100))"),
        "expected html_escape-wrapped truncate; got:\n{src}",
    );
}

#[test]
fn lowered_article_partial_link_to_record_uses_singular_path_helper() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_article.rb");
    // Stage 2 macro-inline: each `link_to text, record, ...` expands
    // to `<a href="<escaped>" class="...">text</a>` inline; no
    // runtime `ViewHelpers.link_to` call survives.
    assert!(
        !src.contains("ViewHelpers.link_to"),
        "ViewHelpers.link_to runtime call should be retired; got:\n{src}",
    );
    // `link_to article.title, article, ...` — URL rewrites to
    // `RouteHelpers.article_path(article.id)` and the text
    // html-escapes through the interp.
    assert!(
        src.contains("ActionView::ViewHelpers.html_escape(RouteHelpers.article_path(article.id))"),
        "expected html_escape on article_path URL; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.html_escape(article.title)"),
        "expected html_escape on article.title link text; got:\n{src}",
    );
    // `link_to "Show", article, ...` — literal text. html_escape of a
    // string literal with no HTML-special chars is constant-folded away
    // at lower time, so it emits the plain literal interpolation.
    assert!(
        src.contains("#{\"Show\"}") && !src.contains("html_escape(\"Show\")"),
        "expected constant-folded Show link text; got:\n{src}",
    );
    // `link_to "Edit", edit_article_path(article), ...` — path-helper
    // URL with bare-local arg → article.id.
    assert!(
        src.contains("RouteHelpers.edit_article_path(article.id)"),
        "expected edit_article_path(article.id); got:\n{src}",
    );
}

#[test]
fn lowered_article_partial_button_to_record_with_options() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_article.rb");
    // Stage 2 macro-inline: `button_to "Destroy", article, method:
    // :delete, ...` expands to the wrapping `<form>` + method
    // override + `<button>` + CSRF inline shape; no runtime
    // `ViewHelpers.button_to` call survives. method peeled out of
    // opts feeds `method_override_input(:delete)`; class + data
    // entries flow as `<button>` attrs (data-turbo-confirm
    // flattens from the nested hash).
    assert!(
        !src.contains("ViewHelpers.button_to"),
        "ViewHelpers.button_to runtime call should be retired; got:\n{src}",
    );
    assert!(
        src.contains("<form action=\\\""),
        "expected inline <form action=...> wrapper; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.html_escape(RouteHelpers.article_path(article.id))"),
        "expected article_path URL in form action; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.method_override_input(:delete)"),
        "expected method_override_input(:delete) call; got:\n{src}",
    );
    assert!(
        src.contains("data-turbo-confirm=\\\""),
        "expected flattened data-turbo-confirm attr; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.csrf_token_hidden_input"),
        "expected csrf_token_hidden_input call; got:\n{src}",
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
        src.contains("def self.form(article, notice = nil, alert = nil)"),
        "expected `def self.form(article, notice, alert)`; got:\n{src}",
    );
}

#[test]
fn lowered_form_partial_form_with_inline_expansion() {
    // `<%= form_with(...) do |form| ... %>` inline-expands fully at
    // lower time (Wedges 1b-i + 1b-ii of the macro-inline
    // retirement): no `ViewHelpers.form_with`, no `FormBuilder.new`,
    // no `form.label(...)` / `form.text_field(...)` runtime calls
    // survive. The lowerer materializes the form bytes end-to-end:
    // `form_method = ...` local + open tag + CSRF/_method runtime
    // helpers + per-input inline HTML + close tag.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_form.rb");
    assert!(
        !src.contains("ViewHelpers.form_with"),
        "ViewHelpers.form_with runtime call should be retired; got:\n{src}",
    );
    assert!(
        !src.contains("FormBuilder.new"),
        "FormBuilder constructor should be retired; got:\n{src}",
    );
    assert!(
        !src.contains("form.label") && !src.contains("form.text_field")
            && !src.contains("form.text_area") && !src.contains("form.submit"),
        "form.X dispatch should be retired; got:\n{src}",
    );
    assert!(
        !src.contains("body = String.new"),
        "form_with inline expansion should not introduce inner `body` accumulator; got:\n{src}",
    );
    // Synthesized `form_method` local binds the method symbol for
    // method_override_input + submit-default-text reads.
    assert!(
        src.contains("form_method = if article.persisted?"),
        "expected form_method local binding via if persisted?; got:\n{src}",
    );
    // For `model: article` (a simple Var), record_var reuses the
    // local — no `form_record = article` redundant binding.
    assert!(
        !src.contains("form_record = article"),
        "should reuse `article` local directly when model is a Var; got:\n{src}",
    );
    // Opening `<form ...>` tag.
    assert!(
        src.contains("<form action=\\\""),
        "expected inline open-form tag; got:\n{src}",
    );
    assert!(
        src.contains("accept-charset=\\\"UTF-8\\\" method=\\\"post\\\""),
        "expected static accept-charset + method=post in open tag; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.method_override_input(form_method)"),
        "expected method_override_input(form_method) call; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.csrf_token_hidden_input"),
        "expected csrf_token_hidden_input call; got:\n{src}",
    );
    // Inline per-input HTML (representative samples). After append-
    // coalescing the literal often lives inside a larger Lit::Str run.
    assert!(
        src.contains("<label for=\\\"article_title\\\">Title</label>"),
        "expected inline label HTML; got:\n{src}",
    );
    assert!(
        src.contains("name=\\\"article[title]\\\" id=\\\"article_title\\\""),
        "expected inline text_field name/id attrs; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.optional_value_attr(article[:title])"),
        "expected optional_value_attr on article[:title]; got:\n{src}",
    );
    assert!(
        src.contains("name=\\\"article[body]\\\" id=\\\"article_body\\\""),
        "expected inline text_area name/id attrs; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.escape_or_empty(article[:body])"),
        "expected escape_or_empty on article[:body] for text_area body; got:\n{src}",
    );
    // Submit with no label: default text branches on form_method.
    assert!(
        src.contains("if form_method == :patch"),
        "expected default submit text conditional on form_method; got:\n{src}",
    );
    assert!(
        src.contains("\"Update Article\""),
        "expected `Update Article` default text branch; got:\n{src}",
    );
    assert!(
        src.contains("\"Create Article\""),
        "expected `Create Article` default text branch; got:\n{src}",
    );
    assert!(
        src.contains("</form>"),
        "expected closing `</form>` literal in body; got:\n{src}",
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
    // After Wedge 1b-ii: form.X dispatch is fully inlined; no
    // runtime form.label/text_field/text_area/submit calls survive.
    // The class-array opt simplification still picks the base +
    // first-key (matching prior runtime behavior for the 5 compare
    // paths) and shows up inline in the rendered class attr.

    // `form.label :title` (no opts) → inline `<label
    // for="article_title">Title</label>`.
    assert!(
        src.contains("<label for=\\\"article_title\\\">Title</label>"),
        "expected inline label HTML; got:\n{src}",
    );
    // `form.text_field :title, class: [...]` → inline `<input
    // type="text" name="article[title]" id="article_title"<value_attr>
    // class="<simplified>">`. Class-array collapse + html_escape on
    // the simplified literal land in the StringInterp.
    assert!(
        src.contains("name=\\\"article[title]\\\" id=\\\"article_title\\\""),
        "expected inline text_field name/id; got:\n{src}",
    );
    assert!(
        src.contains("\"block shadow-sm rounded-md border px-3 py-2 mt-2 w-full border-gray-400 focus:outline-blue-600\""),
        "expected class-array collapsed to base + first hash key; got:\n{src}",
    );
    // `form.textarea :body, rows: 4, ...` → inline
    // `<textarea name="article[body]" id="article_body" rows="4"
    // class="..."><body></textarea>`. The `textarea` alias still
    // normalizes to `text_area` at the classifier; output uses the
    // <textarea> HTML element either way.
    assert!(
        src.contains("name=\\\"article[body]\\\" id=\\\"article_body\\\""),
        "expected inline text_area name/id; got:\n{src}",
    );
    assert!(
        src.contains("</textarea>"),
        "expected closing </textarea> in inline text_area; got:\n{src}",
    );
    // `form.submit class: "..."` (no label) → inline `<input
    // type="submit" ... value="<conditional>" data-disable-with=...>`
    // with the conditional resolving form_method to Update/Create.
    assert!(
        src.contains("<input type=\\\"submit\\\" name=\\\"commit\\\""),
        "expected inline submit input; got:\n{src}",
    );
    assert!(
        src.contains("\"Update Article\"") && src.contains("\"Create Article\""),
        "expected default-text branches for submit; got:\n{src}",
    );
}

#[test]
fn lowered_form_partial_errors_each_iterates_with_html_escape() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/_form.rb");
    // `<% article.errors.each do |error| %>...<% end %>` walks as a
    // template-level each block. After Wedge 1b-i the active
    // accumulator inside the form_with body is the outer `io` (no
    // inner `body =` capture).
    assert!(
        src.contains("article.errors.each do |error|"),
        "expected article.errors.each block; got:\n{src}",
    );
    // `<%= error.full_message %>` becomes just `error` after the
    // errors-each adapter rewrite (spinel-runtime errors are plain
    // Strings, no `full_message` method). Auto-escape still applies.
    // After coalescing, the call typically lives inside a wider
    // `io << "<li>#{...}</li>"` Lit::Str/StringInterp.
    assert!(
        src.contains("ActionView::ViewHelpers.html_escape(error.to_s)"),
        "expected html_escape on `.to_s`-coerced bare `error` (full_message stripped); got:\n{src}",
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
        src.contains("def self.comment(comment, notice = nil, alert = nil)"),
        "expected `def self.comment(comment, notice, alert)`; got:\n{src}",
    );
}

#[test]
fn lowered_comment_partial_nested_url_array_to_path_helper() {
    // `<%= button_to "Delete", [comment.article, comment], method:
    // :delete, ... %>` lowers the nested-resource array to
    // `RouteHelpers.article_comment_path(comment.article_id,
    // comment.id)`. Stage 2 macro-inline: the surrounding button_to
    // expands inline (no runtime ViewHelpers.button_to call); the
    // resolved nested path appears in the form's action attr.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/comments/_comment.rb");
    assert!(
        !src.contains("ViewHelpers.button_to"),
        "ViewHelpers.button_to runtime call should be retired; got:\n{src}",
    );
    assert!(
        src.contains(
            "ActionView::ViewHelpers.html_escape(RouteHelpers.article_comment_path(comment.article_id, comment.id))"
        ),
        "expected nested-array URL → article_comment_path with FK + id in inline form action; got:\n{src}",
    );
}

#[test]
fn lowered_comment_partial_auto_escape_on_attrs() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/comments/_comment.rb");
    // Bare-attr interpolations get html_escape (on the `.to_s`-coerced
    // value) on the way to io.
    assert!(
        src.contains("ActionView::ViewHelpers.html_escape(comment.commenter.to_s)"),
        "expected html_escape(comment.commenter.to_s); got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.html_escape(comment.body.to_s)"),
        "expected html_escape(comment.body.to_s); got:\n{src}",
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
        src.contains("ActionView::ViewHelpers.turbo_stream_from(\"article_#{article.id}_comments\")"),
        "expected turbo_stream_from with interpolated channel; got:\n{src}",
    );
}

#[test]
fn lowered_show_view_form_with_nested_array_model_dispatches_form_builder() {
    // `<%= form_with model: [@article, Comment.new], ... do |form|
    // %>` — polymorphic-array form_with for a nested resource. After
    // Wedges 1b-i + 1b-ii: form_with + form.X all inline-expand at
    // lower time. The non-Var model expression (`Comment.new`) gets
    // synthesized into a `form_record` local; the form_method is
    // the literal `:post` for the always-new child record.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/show.rb");
    assert!(
        !src.contains("ViewHelpers.form_with"),
        "ViewHelpers.form_with runtime call should be retired; got:\n{src}",
    );
    assert!(
        !src.contains("FormBuilder.new"),
        "FormBuilder constructor should be retired; got:\n{src}",
    );
    // Non-Var model → synthesized `form_record` local at the form
    // entry; attribute reads dispatch through it.
    assert!(
        src.contains("form_record = Comment.new"),
        "expected synthesized form_record = Comment.new; got:\n{src}",
    );
    // form_method literal :post (Class.new is never persisted, so
    // no `persisted?` conditional).
    assert!(
        src.contains("form_method = :post"),
        "expected form_method = :post for nested-resource child; got:\n{src}",
    );
    // Nested action lands in the open form tag's action attribute.
    assert!(
        src.contains(
            "ActionView::ViewHelpers.html_escape(RouteHelpers.article_comments_path(article.id))"
        ),
        "expected nested collection path in form action; got:\n{src}",
    );
    // Inline form.X expansions read attributes through form_record.
    assert!(
        src.contains("<label for=\\\"comment_commenter\\\"")
            && src.contains(">Commenter</label>"),
        "expected inline commenter label; got:\n{src}",
    );
    assert!(
        src.contains(
            "name=\\\"comment[commenter]\\\" id=\\\"comment_commenter\\\""
        ),
        "expected inline commenter text_field name/id; got:\n{src}",
    );
    assert!(
        src.contains(
            "ActionView::ViewHelpers.optional_value_attr(form_record[:commenter])"
        ),
        "expected optional_value_attr on form_record[:commenter]; got:\n{src}",
    );
    assert!(
        src.contains("name=\\\"comment[body]\\\" id=\\\"comment_body\\\""),
        "expected inline body text_area name/id; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.escape_or_empty(form_record[:body])"),
        "expected escape_or_empty on form_record[:body]; got:\n{src}",
    );
    // `form.submit "Add Comment", class: "..."` — positional label
    // wins (no default-text conditional); appears as a literal in
    // the submit input.
    // Literal label folds at lower time (no HTML-special chars).
    assert!(
        src.contains("#{\"Add Comment\"}") && !src.contains("html_escape(\"Add Comment\")"),
        "expected constant-folded Add Comment value; got:\n{src}",
    );
    assert!(
        !src.contains("ActionView::ViewHelpers.html_escape(form."),
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
        src.contains("def self.application(body, notice = nil, alert = nil)"),
        "expected `def self.application(body, notice, alert)`; got:\n{src}",
    );
}

#[test]
fn lowered_layout_view_bare_yield_renders_body_local() {
    // `<%= yield %>` (no slot arg) is the layout's body slot — lowers
    // to a reference to `body` (the explicit param), not a ViewHelpers
    // call. After append-coalescing this often interpolates inline as
    // `#{body}` within a wider StringInterp instead of standing alone
    // as `io << body`.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/layouts/application.rb");
    assert!(
        src.contains("#{body}") || src.contains("io << body"),
        "expected `body` interpolation from bare yield; got:\n{src}",
    );
}

#[test]
fn lowered_layout_view_yield_slot_uses_get_slot() {
    // `<%= yield :head %>` → `ViewHelpers.get_slot(:head)`. May land
    // either as a standalone append or interpolated into the
    // surrounding StringInterp post-coalesce.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/layouts/application.rb");
    assert!(
        src.contains("ActionView::ViewHelpers.get_slot(:head)"),
        "expected `ViewHelpers.get_slot(:head)` for yielded slot; got:\n{src}",
    );
}

#[test]
fn lowered_layout_view_head_helpers() {
    // The bare zero-arg layout helpers all dispatch to ViewHelpers.*.
    // Substring (no `io << ` prefix) matches both standalone-append
    // and post-coalesce StringInterp `#{...}` forms.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/layouts/application.rb");
    assert!(
        src.contains("ActionView::ViewHelpers.csrf_meta_tags"),
        "expected csrf_meta_tags rewrite; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.csp_meta_tag"),
        "expected csp_meta_tag rewrite; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.javascript_importmap_tags"),
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
        src.contains("ActionView::ViewHelpers.stylesheet_link_tag(\"application\""),
        "expected `application` stylesheet link; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.stylesheet_link_tag(\"tailwind\""),
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
    // form before the html_escape wrap is added. The `.to_s`
    // coercion wraps the whole BoolOp in parens — without them the
    // `.to_s` would bind only to the `"Real Blog"` right operand.
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/layouts/application.rb");
    assert!(
        src.contains(
            "ActionView::ViewHelpers.html_escape((ActionView::ViewHelpers.content_for_get(:title) || \"Real Blog\").to_s)"
        ),
        "expected nested helper rewrite under parenthesized BoolOp + .to_s + outer html_escape; got:\n{src}",
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
        src.contains("def self.new(article, notice = nil, alert = nil)"),
        "expected `def self.new(article, notice, alert)`; got:\n{src}",
    );
    assert!(
        src.contains("Views::Articles.form(article)"),
        "expected named-partial dispatch to Views::Articles.form(article); got:\n{src}",
    );
}

#[test]
fn lowered_edit_view_dispatches_named_partial_and_record_link() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/edit.rb");
    assert!(
        src.contains("def self.edit(article, notice = nil, alert = nil)"),
        "expected `def self.edit(article, notice, alert)`; got:\n{src}",
    );
    // Same shared partial as `new.html.erb`.
    assert!(
        src.contains("Views::Articles.form(article)"),
        "expected named-partial dispatch; got:\n{src}",
    );
    // `<%= link_to "Show this article", @article, ... %>` — the URL
    // arg is the bare local record (post-ivar-rewrite). Stage 2
    // macro-inline: expands to inline `<a href="<escaped>" ...>text</a>`;
    // the URL still resolves to `RouteHelpers.article_path
    // (article.id)`.
    assert!(
        !src.contains("ViewHelpers.link_to"),
        "ViewHelpers.link_to runtime call should be retired; got:\n{src}",
    );
    assert!(
        src.contains("ActionView::ViewHelpers.html_escape(RouteHelpers.article_path(article.id))"),
        "expected article_path URL through html_escape in inline link; got:\n{src}",
    );
    // Literal label folds at lower time (no HTML-special chars).
    assert!(
        src.contains("#{\"Show this article\"}")
            && !src.contains("html_escape(\"Show this article\")"),
        "expected constant-folded link text; got:\n{src}",
    );
}

// ── app/helpers resolution (Option A) ───────────────────────────

fn ingest_tree(files: &[(&str, &str)]) -> roundhouse::App {
    let tree = files
        .iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree")
}

#[test]
fn app_helper_calls_resolve_to_module_functions() {
    use roundhouse::ident::Symbol;
    // A non-empty `app/helpers` method must emit as a module-function
    // (`def self.shout`) AND a bare `shout(...)` call in a view must be
    // rewritten to `ApplicationHelper.shout(...)`. Rails mixes helpers into
    // the view *instance*, but the emitted view is a module function with no
    // instance to dispatch the bare call on — so the call would otherwise
    // raise NoMethodError (the lobsters `avatar_img` GET / blocker).
    let app = ingest_tree(&[
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        (
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  def shout(s)\n    s.upcase\n  end\nend\n",
        ),
        ("app/views/articles/index.html.erb", "<p><%= shout(\"hi\") %></p>\n"),
    ]);

    // Registry populated from app/helpers.
    assert!(
        app.helper_method_index.contains_key(&Symbol::from("shout")),
        "helper registry should record `shout`",
    );

    // Helper module emits as a module-function.
    let helper_files = ruby::emit_library(&app);
    let helper_src = find(&helper_files, "application_helper.rb");
    assert!(
        helper_src.contains("def self.shout"),
        "helper method must emit as a module-function; got:\n{helper_src}",
    );

    // Bare helper call in the view resolves to the module.
    let view_files = ruby::emit_lowered_views(&app);
    let view_src = find(&view_files, "articles/index.rb");
    assert!(
        view_src.contains("ApplicationHelper.shout("),
        "bare helper call must resolve to ApplicationHelper.shout; got:\n{view_src}",
    );
}

#[test]
fn integer_durations_rewrite_to_duration_calls() {
    // `Integer#days` doesn't exist (no built-in subclassing), so the Ruby
    // emit path rewrites `<int>.days` → `ActiveSupport::Duration.days(<int>)`
    // against the CRuby Duration overlay; `.ago` then rides the instance.
    // A plural unit rewrites unconditionally (handles an untyped Int constant
    // receiver), but a singular `day`/`hour`/`month`/`year` also names a Time
    // component reader, so it rewrites only when the receiver is numeric —
    // `created_at.day` (a datetime, typed Ty::Time) must be left alone.
    let app = ingest_tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.datetime :created_at\n  end\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  WINDOW = 70\n  def recent?\n    created_at > 70.days.ago\n  end\n  def windowed?\n    created_at > WINDOW.days.ago\n  end\n  def created_day\n    created_at.day\n  end\nend\n",
        ),
    ]);
    let files = ruby::emit_lowered_models(&app);
    let src = find(&files, "user.rb");
    assert!(
        src.contains("ActiveSupport::Duration.days(70).ago"),
        "numeric-literal duration rewrites; got:\n{src}",
    );
    assert!(
        src.contains("ActiveSupport::Duration.days(WINDOW).ago"),
        "plural duration rewrites even for an (untyped) constant receiver; got:\n{src}",
    );
    assert!(
        !src.contains("Duration.day(created_at)"),
        "a Time component reader (`created_at.day`) must NOT be rewritten; got:\n{src}",
    );
}

#[test]
fn model_class_constants_are_captured() {
    // A model-level `NAME = value` constant (e.g. `User::NEW_USER_DAYS = 70`)
    // must be emitted so in-body references resolve — the DSL classifier
    // drops it into `ModelBodyItem::Unknown`, and the controller path already
    // captures its own constants; this mirrors that for models.
    let app = ingest_tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :username\n  end\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  NEW_USER_DAYS = 70\nend\n",
        ),
    ]);
    let files = ruby::emit_lowered_models(&app);
    let src = find(&files, "user.rb");
    assert!(
        src.contains("NEW_USER_DAYS = 70"),
        "model constant must be emitted; got:\n{src}",
    );
}

#[test]
fn column_query_predicates_match_rails_semantics() {
    // Rails defines `<col>?` on every column, with type-specific semantics
    // (ActiveRecord query_cast_attribute): boolean → the value's truthiness,
    // numeric → non-zero (`0` is false), string → present (`""`/nil is
    // false). Date/DateTime type as `Ty::Time` but hydrate as TEXT
    // (`column_text` returns `""` for NULL), so their `?` tests the stored
    // text for presence — the same `!nil? && != ""` form as a String.
    let app = ingest_tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n    t.integer :score\n    t.boolean :active\n    t.datetime :deleted_at\n  end\nend\n",
        ),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
    ]);
    let files = ruby::emit_lowered_models(&app);
    let src = find(&files, "widget.rb");
    assert!(src.contains("def active?"), "boolean predicate present; got:\n{src}");
    assert!(
        src.contains("!(@score.nil?) && @score != 0"),
        "numeric predicate is non-zero; got:\n{src}",
    );
    assert!(
        src.contains("!(@name.nil?) && @name != \"\""),
        "string predicate is present (non-empty); got:\n{src}",
    );
    // Temporal columns store under the `<col>_raw` ivar (IR-level
    // storage/accessor split); the predicate reads the stored text.
    assert!(
        src.contains("!(@deleted_at_raw.nil?) && @deleted_at_raw != \"\""),
        "datetime predicate reads the `_raw` storage text; got:\n{src}",
    );
}

#[test]
fn framework_asset_helpers_resolve_in_library_bodies() {
    // `image_tag`/`image_path` called bare from a helper body, and the
    // `ActionController::Base.helpers.image_path(...)` idiom from a model
    // body, must resolve to `ActionView::ViewHelpers.*` — the view-template
    // classifier never reaches helper/model bodies, so a bare framework
    // helper there would otherwise dispatch to the enclosing module and
    // raise (the lobsters `avatar_img`/`avatar_path` GET / chain).
    let app = ingest_tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :username\n  end\nend\n",
        ),
        (
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  def avatar(u)\n    image_tag(u.avatar_path)\n  end\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  def avatar_path\n    ActionController::Base.helpers.image_path(\"/x.png\", skip_pipeline: true)\n  end\nend\n",
        ),
    ]);
    let helper_files = ruby::emit_library(&app);
    let helper_src = find(&helper_files, "application_helper.rb");
    assert!(
        helper_src.contains("ActionView::ViewHelpers.image_tag("),
        "bare image_tag must resolve to ViewHelpers; got:\n{helper_src}",
    );
    let model_files = ruby::emit_lowered_models(&app);
    let model_src = find(&model_files, "user.rb");
    assert!(
        model_src.contains("ActionView::ViewHelpers.image_path("),
        "Base.helpers.image_path must resolve to ViewHelpers; got:\n{model_src}",
    );
    assert!(
        !model_src.contains(".helpers"),
        "the `.helpers` chain must collapse away; got:\n{model_src}",
    );
}

#[test]
fn model_method_keeps_optional_default_param() {
    // A model `def avatar_path(size = 100)` must emit with its default — the
    // method ingester used to collect only required params, so the optional
    // was dropped and `def avatar_path` was left with a body still reading
    // `size`: an ArgumentError at every call that passes one (lobsters GET /).
    let app = ingest_tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :username\n  end\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  def avatar_path(size = 100)\n    \"/avatars/#{username}-#{size}.png\"\n  end\nend\n",
        ),
    ]);
    let files = ruby::emit_lowered_models(&app);
    let src = find(&files, "user.rb");
    assert!(
        src.contains("def avatar_path(size = 100)"),
        "model method must keep its optional default param; got:\n{src}",
    );
}

#[test]
fn empty_app_helper_module_is_a_no_op() {
    // The blog ships empty helper modules (`module ApplicationHelper; end`).
    // They contribute no registry entries, so helper lowering stays a strict
    // no-op and a bare interpolation keeps its plain (un-namespaced) shape.
    let app = ingest_tree(&[
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("app/helpers/application_helper.rb", "module ApplicationHelper\nend\n"),
        ("app/views/articles/index.html.erb", "<p><%= notice %></p>\n"),
    ]);
    assert!(
        app.helper_method_index.is_empty(),
        "empty helper module must add no registry entries",
    );
    let view_files = ruby::emit_lowered_views(&app);
    let view_src = find(&view_files, "articles/index.rb");
    assert!(
        !view_src.contains("ApplicationHelper."),
        "no helper namespacing should appear; got:\n{view_src}",
    );
}
