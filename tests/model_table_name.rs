//! A model's explicit table name binds both its row and its emitted adapters.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::{ingest_app_from_tree, survey};
use roundhouse::ty::Ty;
use roundhouse::Symbol;

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table "items" do |t|
    t.integer "label", null: false
  end
  create_table "archive_items" do |t|
    t.boolean "label", null: false
  end
  create_table "legacy_entries" do |t|
    t.string "label", null: false
    t.string "optional_label"
  end
  create_table "other_entries" do |t|
    t.integer "label", null: false
  end
end
"#;

fn tree(source: &str) -> HashMap<PathBuf, Vec<u8>> {
    [("db/schema.rb", SCHEMA), ("app/models/item.rb", source)]
        .into_iter()
        .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
        .collect()
}

fn assert_unbound(files: HashMap<PathBuf, Vec<u8>>) {
    let source = &files[&PathBuf::from("app/models/item.rb")];
    let parsed = ruby_prism::parse(source);
    let parse_errors: Vec<_> = parsed.errors().map(|e| e.message().to_string()).collect();
    assert!(parse_errors.is_empty(), "invalid repro: {parse_errors:?}");
    drop(parsed);

    let error = ingest_app_from_tree(files.clone()).expect_err("unsafe binding must be refused");
    assert!(error.to_string().contains("table_name binding"), "{error}");

    survey::activate();
    let result = ingest_app_from_tree(files);
    let gaps = survey::drain();
    let app = result.expect("survey records the unbound model");
    assert!(
        app.models.is_empty(),
        "do not bind the model to a guessed table"
    );
    assert_eq!(gaps.len(), 1);
    assert!(
        gaps[0].to_string().contains("table_name binding"),
        "{gaps:?}"
    );
}

#[test]
fn a_literal_table_name_overrides_convention_and_namespace_prefix() {
    for source in [
        "class Item < ApplicationRecord\n  self.table_name = \"legacy_entries\"\nend\n",
        "class Item < ApplicationRecord\n  self.table_name = :legacy_entries\nend\n",
        r#"class Item < ApplicationRecord
  self.table_name = :"legacy\x5fentries"
end
"#,
        r#"module Archive
  def self.table_name_prefix
    "archive_"
  end
  class Item < ApplicationRecord
    self.table_name = "legacy_entries"
  end
end
"#,
        r#"module Archive
  def self.table_name_prefix
    "archive_"
  end
  class Item < ApplicationRecord
    self.table_name = :legacy_entries
  end
end
"#,
    ] {
        let app = ingest_app_from_tree(tree(source)).expect("literal table override");
        let model = &app.models[0];
        assert_eq!(model.table.0.as_str(), "legacy_entries");
        assert_eq!(model.attributes.fields[&Symbol::from("label")], Ty::Str);
        assert_eq!(
            model.attributes.fields[&Symbol::from("optional_label")],
            Ty::Union {
                variants: vec![Ty::Str, Ty::Nil]
            },
            "the override does not erase column nullability"
        );
        assert!(
            model.body.is_empty(),
            "the setter is consumed, not replayed: {:?}",
            model.body
        );
    }
}

#[test]
fn conventional_names_and_prefixes_keep_their_binding() {
    for (source, table, label_ty) in [
        ("class Item < ApplicationRecord\nend\n", "items", Ty::Int),
        (
            "module Archive\n  def self.table_name_prefix; \"archive_\"; end\n  class Item < ApplicationRecord\n  end\nend\n",
            "archive_items",
            Ty::Bool,
        ),
    ] {
        let app = ingest_app_from_tree(tree(source)).expect("existing naming rules");
        let model = &app.models[0];
        assert_eq!(model.table.0.as_str(), table);
        assert_eq!(model.attributes.fields[&Symbol::from("label")], label_ty);
    }
}

#[test]
fn unsupported_table_writes_are_ledgered_instead_of_guessed() {
    for declaration in [
        "self.table_name = ENV.fetch(\"TABLE\")",
        "self.table_name = nil",
        "self.table_name = \"legacy_#{suffix}\"",
        "self.table_name = :\"legacy_#{suffix}\"",
        "self.table_name = :\"public.legacy_entries\"",
        "self.table_name = \"public.legacy_entries\"",
        "self.table_name = \"legacy_entries\"\nself.table_name = \"other_entries\"",
        "self.table_name = :legacy_entries\nself.table_name = :other_entries",
        "self.table_name = \"legacy_entries\"\nself.table_name = ENV.fetch(\"TABLE\")",
        "self.table_name ||= \"legacy_entries\"",
        "self.table_name &&= \"legacy_entries\"",
        "self.table_name += \"legacy_entries\"",
        "self.table_name, other = \"legacy_entries\", 1",
        "self.table_name = \"legacy_entries\" if enabled?",
        "configure { self.table_name = \"legacy_entries\" }",
        "Other.table_name = \"legacy_entries\"",
        "self&.table_name = \"legacy_entries\"",
        "self.table_name = \"legacy_entries\", \"other_entries\"",
        "self.table_name = *[\"legacy_entries\"]",
        "class << self; self.table_name = \"legacy_entries\"; end",
    ] {
        let source = format!("class Item < ApplicationRecord\n  {declaration}\nend\n");
        assert_unbound(tree(&source));
    }
}

#[test]
fn safe_bare_names_preserve_case_underscores_and_digits() {
    for name in ["LegacyEntries", "_legacy_entries", "legacy_entries_2"] {
        let source = format!("class Item < ApplicationRecord\n  self.table_name = {name:?}\nend\n");
        let schema = format!(
            "ActiveRecord::Schema.define(version: 1) do\n  create_table {name:?} do |t|\n    t.string \"label\", null: false\n  end\nend\n"
        );
        let mut files = tree(&source);
        files.insert(PathBuf::from("db/schema.rb"), schema.into_bytes());
        let app = ingest_app_from_tree(files).expect("safe bare identifier");
        let model = &app.models[0];
        assert_eq!(model.table.0.as_str(), name);
        assert_eq!(model.attributes.fields[&Symbol::from("label")], Ty::Str);
        assert!(model.body.is_empty());
    }
}

#[test]
fn table_names_that_need_sql_quoting_are_ledgered() {
    for name in [
        "order",
        "OrDeR",
        "legacy-entries",
        "legacy entries",
        "legacy\"entries",
        "123_entries",
        "",
    ] {
        for literal in [format!("{name:?}"), format!(":{name:?}")] {
            let source = format!("class Item < ApplicationRecord\n  self.table_name = {literal}\nend\n");
            let schema = format!(
                "ActiveRecord::Schema.define(version: 1) do\n  create_table {name:?} do |t|\n    t.string \"label\"\n  end\nend\n"
            );
            let mut files = tree(&source);
            files.insert(PathBuf::from("db/schema.rb"), schema.into_bytes());
            assert_unbound(files);
        }
    }
    assert_unbound(tree("class Item < ApplicationRecord\n  self.table_name = :\"ord\\u0065r\"\nend\n"));
}

#[test]
fn executed_declaration_headers_cannot_hide_table_writes() {
    for header in [
        "class Nested < (self.table_name = \"other_entries\"; Object); end",
        "class (self.table_name = \"other_entries\"; Object)::Nested; end",
        "module (self.table_name = \"other_entries\"; Object)::Namespace; end",
        "def (self.table_name = \"other_entries\").probe; end",
    ] {
        for earlier in ["", "self.table_name = \"legacy_entries\"\n"] {
            let source = format!("class Item < ApplicationRecord\n  {earlier}{header}\nend\n");
            assert_unbound(tree(&source));
        }
    }
}

#[test]
fn method_and_nested_class_writes_do_not_bind_the_enclosing_model() {
    let app = ingest_app_from_tree(tree(
        r#"class Item < ApplicationRecord
  self.table_name = "legacy_entries"
  def self.change_table
    self.table_name = "other_entries"
  end
  def change_table_later(value = self.table_name = "default_entries")
    value
  end
  class Nested
    self.table_name = "nested_items"
  end
  module Namespace
    self.table_name = "namespace_items"
  end
end
"#,
    ))
    .expect("writes in other scopes are not declarations of Item's table");
    let model = &app.models[0];
    assert_eq!(model.table.0.as_str(), "legacy_entries");
    assert_eq!(model.attributes.fields[&Symbol::from("label")], Ty::Str);
    assert!(
        model
            .body
            .iter()
            .any(|item| matches!(item, roundhouse::dialect::ModelBodyItem::Method { .. })),
        "the method is not consumed as a declaration"
    );
}
