//! One `has_many :through` join decision.
//!
//! `Link` belongs to `Article` twice. Matching the source `belongs_to`
//! by target class picks `source_article` for both collections. The
//! writer, the reader, and `dependent: :destroy` have to follow the
//! source association's name, and `initialize` may set `_stale` only
//! when that same decision synthesized a writer.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_models_with_registry;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
  end
  create_table "links", force: :cascade do |t|
    t.integer "source_article_id", null: false
    t.integer "target_article_id", null: false
  end
end
"#,
        ),
        (
            "app/models/article.rb",
            r#"class Article < ApplicationRecord
  has_many :outgoing_links, class_name: "Link", foreign_key: :source_article_id
  has_many :incoming_links, class_name: "Link", foreign_key: :target_article_id
  has_many :linked_articles, through: :outgoing_links, source: :target_article, dependent: :destroy
  has_many :linking_articles, through: :incoming_links, source: :source_article, dependent: :destroy
end
"#,
        ),
        (
            "app/models/link.rb",
            r#"class Link < ApplicationRecord
  belongs_to :source_article, class_name: "Article"
  belongs_to :target_article, class_name: "Article"
end
"#,
        ),
    ]))
    .expect("ingest two-belongs_to through app")
}

fn article_class() -> roundhouse::dialect::LibraryClass {
    let app = app();
    let (lcs, _) = lower_models_with_registry(&app.models, &app.schema, vec![]);
    lcs.into_iter()
        .find(|c| c.name.0.as_str() == "Article")
        .expect("Article")
}

fn method_body(class: &roundhouse::dialect::LibraryClass, name: &str) -> String {
    let method = class
        .methods
        .iter()
        .find(|m| m.name.as_str() == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} missing: {:?}",
                class
                    .methods
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect::<Vec<_>>()
            )
        });
    format!("{:?}", method.body)
}

#[test]
fn the_writer_uses_the_named_source_key() {
    let article = article_class();
    let linked = method_body(&article, "_sync_linked_articles");
    // Class-match would write `source_article_id` on both sides: it is
    // the first belongs_to whose target is Article.
    assert!(
        linked.contains("target_article_id="),
        "linked_articles must write the target_article key:\n{linked}"
    );
    let linking = method_body(&article, "_sync_linking_articles");
    assert!(
        linking.contains("source_article_id="),
        "linking_articles must write the source_article key:\n{linking}"
    );
}

#[test]
fn dependent_destroy_uses_that_join() {
    let article = article_class();
    let before = method_body(&article, "before_destroy");
    assert!(before.contains("Link"), "destroy the join model:\n{before}");
    assert!(
        before.contains("source_article_id") && before.contains("target_article_id"),
        "each collection names both of its join keys:\n{before}"
    );
    assert!(
        before.contains("linked_articles") && before.contains("linking_articles"),
        "destroy walks the reader, so a scope on the association applies:\n{before}"
    );
    assert!(
        before.contains("__row") && before.contains("destroy"),
        "destroy the join row, not the target:\n{before}"
    );
}

#[test]
fn the_reader_joins_the_named_source_key() {
    let files = ruby::emit_lowered_models(&app());
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("article.rb"))
        .map(|f| f.content.clone())
        .expect("emitted article");
    assert!(
        src.contains("INNER JOIN links ON links.target_article_id = articles.id"),
        "linked_articles reader must join target_article_id:\n{src}"
    );
    assert!(
        src.contains("INNER JOIN links ON links.source_article_id = articles.id"),
        "linking_articles reader must join source_article_id:\n{src}"
    );
}

#[test]
fn initialize_sets_stale_only_when_the_decision_writes() {
    let article = article_class();
    let init = method_body(&article, "initialize");
    assert!(
        init.contains("linked_articles_stale"),
        "a writer needs its flag:\n{init}"
    );
    assert!(
        init.contains("linking_articles_stale"),
        "a writer needs its flag:\n{init}"
    );
}

/// The initialize skip reads `ThroughJoins::writer`. A second join
/// matcher in that block — another `through_writer_join` call, or a
/// `belongs_to` / foreign-key scan — is the drift this pin refuses.
#[test]
fn schema_stale_skip_does_not_match_the_join() {
    let src = include_str!("../src/lower/model_to_library/schema.rs");
    let start = src
        .find("through_joins.writer")
        .expect("stale skip reads the shared decision");
    let window = &src[start..(start + 600).min(src.len())];
    assert!(!window.contains("through_writer_join"), "{window}");
    assert!(!window.contains("BelongsTo"), "{window}");
    assert!(!window.contains("foreign_key"), "{window}");
}
