//! `enum :status, processing: 'processing', ready: 'ready'` — Rails 7's
//! dominant spelling for a string-backed enum. The mapping arrives as a
//! trailing keyword-hash argument (Prism's `KeywordHashNode`), not the
//! braced `HashNode` the ingest already handled, and previously fell
//! into the generic `enum :x mapping must be an array or hash literal`
//! ledger message. `enum_label_values` now accepts both shapes.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::expr::{ExprNode, Literal};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_model_to_library_class;
use roundhouse::Symbol;

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
    t.integer "priority", default: 17, null: false
  end
end
"#;

const MODEL: &str = r#"class Article < ApplicationRecord
  enum :status, processing: 'processing', ready: 'ready'
  enum :priority, pending: 17, priority: 41
end
"#;

/// The trailing-keyword-hash mapping ingests at all (no error), and
/// retains its own stored values, not array-position indices. Predicates
/// compare public labels, including one that collides with its column.
#[test]
fn trailing_keyword_hash_mapping_ingests_with_its_own_values() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/article.rb", MODEL),
    ])).expect("ingest");
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert_eq!(model.enums[&Symbol::from("priority")], vec![
        ("pending".into(), Literal::Int { value: 17 }),
        ("priority".into(), Literal::Int { value: 41 }),
    ]);
    let article = lower_model_to_library_class(model, &app.schema);
    for (label, column) in [("processing", "status"), ("ready", "status"), ("pending", "priority"), ("priority", "priority")] {
        let name = format!("{label}?");
        let predicates: Vec<_> = article.methods.iter().filter(|m| m.name.as_str() == name).collect();
        assert_eq!(predicates.len(), 1, "{name} must not be shadowed by a column predicate");
        let ExprNode::Send { recv: Some(recv), method, args, .. } = &*predicates[0].body.node else {
            panic!("{name} must compare the public label, not column truthiness");
        };
        assert_eq!(method.as_str(), "==");
        assert!(matches!(&*recv.node, ExprNode::Send { method, .. } if method.as_str() == column));
        assert_eq!(args.len(), 1);
        assert!(matches!(&*args[0].node, ExprNode::Lit { value: Literal::Str { value } } if value == label));
    }
}
