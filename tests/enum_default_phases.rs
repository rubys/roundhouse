//! Enum declaration defaults survive canonical mapping expansion.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::expr::Literal;
use roundhouse::ident::Symbol;
use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn both_declaration_forms_preserve_the_default_label() {
    for declaration in [
        "enum :priority, { low: -3, high: 7 }, default: :high",
        "enum priority: { low: -3, high: 7 }, _default: :high",
        "enum :priority, { low: -3, high: 7 }, default: \"high\"",
    ] {
        let files = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.integer \"priority\", default: -3\n  end\nend\n".to_string()),
            ("app/models/article.rb", format!("class Article < ApplicationRecord\n  {declaration}\nend\n")),
        ];
        let tree: HashMap<PathBuf, Vec<u8>> = files
            .into_iter()
            .map(|(path, source)| (PathBuf::from(path), source.into_bytes()))
            .collect();
        let app = ingest_app_from_tree(tree).expect("ingest default declaration");
        let article = app
            .models
            .iter()
            .find(|m| m.name.0.as_str() == "Article")
            .unwrap();
        assert_eq!(
            article.enum_defaults.get(&Symbol::from("priority")),
            Some(&Literal::Int { value: 7 }),
            "{declaration}"
        );
    }
}

#[test]
fn a_serialized_default_keeps_its_stored_value() {
    let files = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.string \"severity\", default: \"mild\"\n  end\nend\n"),
        ("app/services/rating.rb", "class Rating < T::Enum\n  enums do\n    High = new(\"severe\")\n    Low = new(\"mild\")\n  end\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\n  enum :severity, Rating.values.to_h { |v| [v.serialize, v.serialize] }, default: :severe\nend\n"),
    ];
    let tree = files
        .into_iter()
        .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest serialized default");
    let article = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Article")
        .unwrap();
    assert_eq!(
        article.enum_defaults.get(&Symbol::from("severity")),
        Some(&Literal::Str {
            value: "severe".into()
        })
    );
}
