//! `a && b` is `a` only when `a` was falsy.
//!
//! `&.` reads as `a && a.m`, and the safe-navigation idiom is everywhere:
//!
//! ```ruby
//! def title_length
//!   (title && title.length) || 0
//! end
//! ```
//!
//! When `title` is truthy the expression is `title.length`; when it is falsy
//! the expression is `title` itself, which can only be `nil` (or `false`). The
//! result was typed as the union of BOTH operands including the string that
//! is never what stops the chain, so `(title && title.length) || 0` was
//! `String | Integer` and `title_length > 1` a mismatch. Only the falsy arms
//! of the left operand belong in the result.

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};

fn binops(model: &str) -> Vec<String> {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        ("app/models/thing.rb", model),
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "things", force: :cascade do |t|
    t.string "title"
  end
end
"#,
        ),
    ]
    .iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app)
        .into_iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::IncompatibleBinop { .. }))
        .map(|d| format!("{:?}", d.kind))
        .collect()
}

#[test]
fn a_safe_navigation_chain_defaulted_with_or_is_the_chains_value() {
    let found = binops(
        r#"class Thing < ApplicationRecord
  def long?
    ((title && title.length) || 0) > 1
  end
  def short?
    (title&.length || 0) < 1
  end
end
"#,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn the_nil_from_a_stopped_chain_still_reaches_the_comparison() {
    let found = binops(
        "class Thing < ApplicationRecord\n  def long?\n    (title && title.length) > 1\n  end\nend\n",
    );
    assert_eq!(found.len(), 1, "{found:?}");
}
