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
