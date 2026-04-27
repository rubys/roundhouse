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
