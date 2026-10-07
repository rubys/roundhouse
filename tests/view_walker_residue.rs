//! The view walker's catch-all files what it drops, and `cache` blocks
//! are transparent rather than dropped.
//!
//! A template statement the walker cannot lower becomes `io << ""` so
//! the emitted view still parses. That is a reasonable fallback and was
//! a terrible ledger: the explanatory `tag` was discarded, so this was
//! the one place in the pipeline where modeling debt left no trace at
//! all.
//!
//! `cache` is what the restored ledger found. Rails' fragment-cache
//! block is a pure optimization wrapper whose BODY is the page, and it
//! fell through to the catch-all — taking the body with it. Two whole
//! lobsters templates (`users/tree.html.erb`, 44 lines, and
//! `users/list.html.erb`) emitted `io << ""` and rendered blank, with
//! nothing anywhere reporting it.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::diagnostic::DiagnosticKind;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app(index: &str) -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
  end
end
"#,
        ),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            r#"class ArticlesController < ApplicationController
  def index
    @articles = Article.all
  end
end
"#,
        ),
        ("app/views/articles/index.html.erb", index),
    ]))
    .expect("ingest")
}

fn emit(index: &str) -> (String, Vec<roundhouse::diagnostic::Diagnostic>) {
    let app = app(index);
    let (files, diags) = roundhouse::emit::diagnostics::scope(|| ruby::emit_lowered_views(&app));
    let body = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/views/articles/index.rb"))
        .map(|f| f.content.clone())
        .expect("index.rb");
    (body, diags)
}

fn residues(diags: &[roundhouse::diagnostic::Diagnostic]) -> Vec<&roundhouse::diagnostic::Diagnostic> {
    diags
        .iter()
        .filter(|d| matches!(&d.kind, DiagnosticKind::LowerResidue { pass, .. }
            if pass.as_str() == "view_walker"))
        .collect()
}

/// A `cache` block renders its body. Serving it from a store is the
/// only thing lost, and nothing in an emitted tree has a store.
#[test]
fn a_cache_block_renders_its_body() {
    let (body, diags) = emit(
        "<% cache [@articles] do %>\n<h1>Articles</h1>\n<% end %>\n",
    );
    assert!(body.contains("<h1>Articles</h1>"), "cache body survives:\n{body}");
    assert!(
        residues(&diags).is_empty(),
        "a recognized cache block is not residue: {diags:?}"
    );
}

/// Without the transparent arm this was the whole story: body gone,
/// nothing reported. Guards the regression in the shape that bit
/// lobsters — the cache block wrapping the ENTIRE template.
#[test]
fn a_cache_wrapped_template_is_not_emitted_empty() {
    let (body, _) = emit(
        "<% cache [@articles] do %>\n<div class=\"box\">\n  <p>one</p>\n  <p>two</p>\n</div>\n<% end %>\n",
    );
    assert!(body.contains("<p>one</p>") && body.contains("<p>two</p>"), "{body}");
    assert!(
        !body.contains("io << \"\"\n      io\n"),
        "not an empty-append stub:\n{body}"
    );
}

/// The ledger itself: a statement the walker genuinely cannot lower
/// still degrades to an empty append, but now says so, with the
/// template position and the shape tag.
#[test]
fn an_unlowerable_statement_files_residue() {
    let (_, diags) = emit("<% a, b = compute_pair %>\n<p>x</p>\n");
    let res = residues(&diags);
    assert_eq!(res.len(), 1, "exactly one residue line: {diags:?}");
    assert!(
        matches!(&res[0].kind, DiagnosticKind::LowerResidue { construct, .. }
            if construct.as_str() == "unknown stmt"),
        "carries the shape tag: {:?}",
        res[0].kind
    );
    assert!(
        res[0].message.contains("dropped"),
        "message names the drop: {}",
        res[0].message
    );
}

/// `<% unless cond %>…<% end %>` reaches the walker as `if cond then nil
/// else … end`: the `then` arm is a SYNTHESIZED bare `nil`, with no span.
/// A literal in statement position renders nothing and does nothing, so
/// there is nothing to drop — but it fell to the catch-all, which filed a
/// span-less "template statement dropped" line for every `unless` in a
/// template. A ledger that cries wolf hides the real drops.
#[test]
fn an_unless_block_files_no_residue_and_keeps_its_body() {
    let (body, diags) = emit("<% unless @articles.empty? %>\n<p>listed</p>\n<% end %>\n");
    assert!(body.contains("<p>listed</p>"), "unless body survives:\n{body}");
    assert!(residues(&diags).is_empty(), "no residue for a bare literal arm: {diags:?}");
}

/// The same holds for a literal written in the template itself.
#[test]
fn a_literal_statement_files_no_residue() {
    let (_, diags) = emit("<% nil %>\n<p>x</p>\n");
    assert!(residues(&diags).is_empty(), "{diags:?}");
}

/// `cached: true` is Rails' collection cache. Campfire's room/messages
/// pages write it; dropping the kwarg walked every partial on every GET.
#[test]
fn a_cached_true_collection_reads_the_store() {
    let tree = tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        ),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            r#"class ArticlesController < ApplicationController
  def index
    @articles = Article.all
  end
end
"#,
        ),
        (
            "app/views/articles/_article.html.erb",
            "<% cache article do %><p><%= article.title %></p><% end %>\n",
        ),
        (
            "app/views/articles/index.html.erb",
            "<%= render partial: \"articles/article\", collection: @articles, cached: true %>\n",
        ),
    ]);
    let app = ingest_app_from_tree(tree).expect("ingest");
    let (files, diags) = roundhouse::emit::diagnostics::scope(|| ruby::emit_lowered_views(&app));
    let body = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/views/articles/index.rb"))
        .map(|f| f.content.clone())
        .expect("index.rb");
    assert!(
        body.contains("views/coll/"),
        "collection cache key:\n{body}"
    );
    assert!(
        body.contains("articles/article"),
        "collection cache key includes the partial:\n{body}"
    );
    assert!(
        body.contains("read_str"),
        "warm path is one store read:\n{body}"
    );
    assert!(
        body.contains("cache_key_with_version"),
        "key walks each record:\n{body}"
    );
    let gate = roundhouse::lower::MAX_UNCACHED_COLLECTION_LENGTH;
    assert!(
        body.contains(&format!(".length > {gate}")),
        "small collections skip the store (cost gate):\n{body}"
    );
    assert!(
        body.contains("__cc_collection_"),
        "collection expression is bound once before the gate:\n{body}"
    );
    assert!(
        residues(&diags).is_empty(),
        "cached: true is not residue: {diags:?}"
    );
}

#[test]
fn a_cached_true_collection_with_locals_puts_them_in_the_key() {
    let tree = tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        ),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            r#"class ArticlesController < ApplicationController
  def index
    @articles = Article.all
    @mode = "card"
  end
end
"#,
        ),
        (
            "app/views/articles/_article.html.erb",
            "<p><%= article.title %>-<%= mode %></p>\n",
        ),
        (
            "app/views/articles/index.html.erb",
            "<%= render partial: \"articles/article\", collection: @articles, cached: true, locals: { mode: @mode } %>\n",
        ),
    ]);
    let app = ingest_app_from_tree(tree).expect("ingest");
    let (files, diags) = roundhouse::emit::diagnostics::scope(|| ruby::emit_lowered_views(&app));
    let body = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/views/articles/index.rb"))
        .map(|f| f.content.clone())
        .expect("index.rb");
    assert!(
        body.contains("mode="),
        "local name belongs in the collection cache key:\n{body}"
    );
    assert!(
        body.contains("cache_key_with_version") && body.contains("inspect"),
        "locals use cache_key_with_version or inspect, not bare to_s:\n{body}"
    );
    assert!(
        !body.contains(".to_s"),
        "locals must not use bare to_s in the key:\n{body}"
    );
    assert!(
        residues(&diags).is_empty(),
        "cached: true locals is not residue: {diags:?}"
    );
}

#[test]
fn a_cached_true_collection_with_slash_locals_keeps_names_in_the_key() {
    let tree = tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        ),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            r#"class ArticlesController < ApplicationController
  def index
    @articles = Article.all
  end
end
"#,
        ),
        (
            "app/views/articles/_article.html.erb",
            "<p><%= article.title %>-<%= left %>-<%= right %></p>\n",
        ),
        (
            "app/views/articles/index.html.erb",
            "<%= render partial: \"articles/article\", collection: @articles, cached: true, locals: { left: \"a/b\", right: \"c\" } %>\n",
        ),
    ]);
    let app = ingest_app_from_tree(tree).expect("ingest");
    let (files, diags) = roundhouse::emit::diagnostics::scope(|| ruby::emit_lowered_views(&app));
    let body = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/views/articles/index.rb"))
        .map(|f| f.content.clone())
        .expect("index.rb");
    assert!(
        body.contains("left=") && body.contains("right="),
        "both local names in the key:\n{body}"
    );
    assert!(
        residues(&diags).is_empty(),
        "slash locals is not residue: {diags:?}"
    );
}

#[test]
fn a_cached_true_collection_puts_threaded_ivar_context_in_the_key() {
    let tree = tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
    t.datetime "updated_at", null: false
  end
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#,
        ),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            r#"class ArticlesController < ApplicationController
  def index
    @articles = Article.all
    @viewer = User.first
  end
end
"#,
        ),
        (
            "app/views/articles/_article.html.erb",
            "<p><%= article.title %>-<%= @viewer.name %></p>\n",
        ),
        (
            "app/views/articles/index.html.erb",
            "<%= render partial: \"articles/article\", collection: @articles, cached: true %>\n",
        ),
    ]);
    let app = ingest_app_from_tree(tree).expect("ingest");
    let (files, diags) = roundhouse::emit::diagnostics::scope(|| ruby::emit_lowered_views(&app));
    let body = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/views/articles/index.rb"))
        .map(|f| f.content.clone())
        .expect("index.rb");
    assert!(
        body.contains("viewer="),
        "threaded closure ivar belongs in the collection cache key:\n{body}"
    );
    assert!(
        residues(&diags).is_empty(),
        "threaded-ivar cached: true is not residue: {diags:?}"
    );
}
