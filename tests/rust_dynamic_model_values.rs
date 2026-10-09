//! Dynamic Active Record APIs must box heterogeneous values as JSON in Rust.

use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app_from_tree;
use std::collections::HashMap;
use std::path::PathBuf;

#[test]
fn attributes_and_index_reads_convert_column_values_to_json() {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :name, null: false\n    t.integer :visits, null: false\n    t.string :nickname\n  end\nend\n",
        ),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
    ]
    .into_iter()
    .map(|(path, content)| (PathBuf::from(path), content.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);

    let source = rust::emit(&app)
        .into_iter()
        .find(|file| file.path.to_string_lossy().ends_with("widget.rs"))
        .expect("Widget model output")
        .content;
    let attributes = source
        .split("pub fn attributes(&self)")
        .nth(1)
        .expect("attributes method")
        .split("\n    pub fn ")
        .next()
        .unwrap();
    let get_index = source
        .split("pub fn get_index(&self, name: &str)")
        .nth(1)
        .expect("index read method")
        .split("\n    pub fn ")
        .next()
        .unwrap();

    assert!(
        attributes.contains("serde_json::Value::from(self.name.clone())"),
        "non-null String column must be boxed:\n{attributes}"
    );
    assert!(
        attributes.contains("serde_json::Value::from(self.visits)"),
        "Integer column must be boxed:\n{attributes}"
    );
    assert!(
        attributes.contains("serde_json::Value::from(self.nickname.clone())"),
        "nullable String column must be boxed without losing None:\n{attributes}"
    );
    for field in ["name", "visits", "nickname"] {
        assert!(
            get_index.contains(&format!("serde_json::Value::from(self.{field}")),
            "heterogeneous index arm {field} must return JSON Value:\n{get_index}"
        );
    }
    assert!(get_index.contains("_ => serde_json::Value::Null"), "{get_index}");
}
