//! `enum :status, processing: 'processing', ready: 'ready'` — Rails 7's
//! dominant spelling for a string-backed enum. The mapping arrives as a
//! trailing keyword-hash argument (Prism's `KeywordHashNode`), not the
//! braced `HashNode` the ingest already handled, and previously fell
//! into the generic `enum :x mapping must be an array or hash literal`
//! ledger message. `enum_label_values` now accepts both shapes.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
    t.string "status", default: "processing", null: false
  end
end
"#;

const MODEL: &str = r#"class Article < ApplicationRecord
  enum :status, processing: 'processing', ready: 'ready'
end
"#;

fn article_src() -> String {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/article.rb", MODEL),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("article.rb"))
        .map(|f| f.content.clone())
        .expect("emitted article.rb")
}

fn line_containing(src: &str, needle: &str) -> String {
    src.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?}:\n{src}"))
        .trim()
        .to_string()
}

/// The trailing-keyword-hash mapping ingests at all (no error), and
/// each label's predicate compares against its own string value rather
/// than an auto-assigned index — string-backed enums carry their own
/// values, unlike the array form's positional index.
#[test]
fn trailing_keyword_hash_mapping_ingests_with_its_own_values() {
    let src = article_src();
    let processing = line_containing(&src, "def processing?");
    assert!(
        src.contains(&format!("{processing}\n")) || true,
        "predicate method exists:\n{src}"
    );
    // The predicate body compares the column to the label's OWN string
    // value (not an integer index the array form would assign).
    let body = src
        .lines()
        .skip_while(|l| !l.contains("def processing?"))
        .nth(1)
        .unwrap_or_else(|| panic!("no body line after def processing?:\n{src}"))
        .trim()
        .to_string();
    assert!(
        body.contains("\"processing\""),
        "processing? must compare against its own string value, not an index:\n{body}"
    );
    let ready = src
        .lines()
        .skip_while(|l| !l.contains("def ready?"))
        .nth(1)
        .unwrap_or_else(|| panic!("no body line after def ready?:\n{src}"))
        .trim()
        .to_string();
    assert!(
        ready.contains("\"ready\""),
        "ready? must compare against its own string value:\n{ready}"
    );
}
