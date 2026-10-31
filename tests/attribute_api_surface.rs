//! PostgreSQL: attribute api surface, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.0].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name", limit: 40
    t.integer "shop_id"
    t.text "notes"
    t.boolean "live"
    t.decimal "price"
    t.decimal "cost", precision: 10, scale: 2
  end
end
"#;

const CONCERN: &str = r#"module UuidColumns
  extend ActiveSupport::Concern

  class_methods do
    def uuid_attributes(*names)
      names.each { |name| UuidColumns.register(self, name.to_s) }
    end
  end

  def self.register(model, name)
    name
  end
end
"#;

fn emitted() -> Vec<(String, String)> {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("roundhouse.yml", ""),
        ("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_spec_attribute_api\n"),
        ("app/models/concerns/uuid_columns.rb", CONCERN),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\n  include UuidColumns\nend\n",
        ),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  uuid_attributes :shop_id\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .into_iter()
        .map(|f| (f.path.to_string_lossy().to_string(), f.content))
        .collect()
}

fn file<'a>(files: &'a [(String, String)], suffix: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .map(|(_, c)| c.as_str())
        .unwrap_or_else(|| panic!("no {suffix} in {:?}", files.iter().map(|(p, _)| p).collect::<Vec<_>>()))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_abstract_class_write_and_an_inherited_dsl_call_stay_in_the_class_body() {
    let files = emitted();
    let base = file(&files, "application_record.rb");
    assert!(base.contains("  self.abstract_class = true\n"), "{base}");
    let widget = file(&files, "widget.rb");
    assert!(
        widget.contains("uuid_attributes(:shop_id)") || widget.contains("uuid_attributes :shop_id"),
        "{widget}"
    );
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_schema_column_types_follow_the_postgresql_type_map() {
    let files = emitted();
    let widget = file(&files, "widget.rb");
    let start = widget.find("def self.schema_column_types").unwrap_or_else(|| panic!("{widget}"));
    let body = &widget[start..];
    let body = &body[..body.find("\n  end").unwrap_or(body.len())];
    for entry in [
        r#""id" => [:integer, 8]"#,
        r#""name" => [:string, 40]"#,
        r#""shop_id" => [:integer, 4]"#,
        r#""notes" => [:text, nil]"#,
        r#""live" => [:boolean, nil]"#,
        // OID::Decimal's precision and scale, nil where the column
        // declares neither (native `type_for_attribute`).
        r#""price" => [:decimal, nil, nil]"#,
        r#""cost" => [:decimal, 10, 2]"#,
    ] {
        assert!(body.contains(entry), "{entry} missing from {body}");
    }
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_record_reader_accepts_a_string_name() {
    let files = emitted();
    let widget = file(&files, "widget.rb");
    let start = widget.find("def [](name)").unwrap_or_else(|| panic!("{widget}"));
    assert!(widget[start..].contains("name = name.to_sym if name.is_a?(String)"), "{widget}");
    let start = widget.find("def []=(name, value)").unwrap_or_else(|| panic!("{widget}"));
    assert!(widget[start..].contains("name = name.to_sym if name.is_a?(String)"), "{widget}");
}
