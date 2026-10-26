//! A column named after an SQL keyword is quoted wherever roundhouse
//! writes it into SQL: the DDL, the SELECT lists, INSERT and UPDATE.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

/// The spinel tree for a small app: the two base classes plus `files`.
#[allow(dead_code)]
fn spinel(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("app/models/application_record.rb"), APPLICATION_RECORD.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        APPLICATION_CONTROLLER.as_bytes().to_vec(),
    );
    for (path, content) in files {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files")
}

#[allow(dead_code)]
fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not emitted"))
        .1
}

#[allow(dead_code)]
fn assert_parses(files: &[(String, String)], path: &str) {
    let source = file(files, path);
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| String::from_utf8_lossy(e.message()).into_owned()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do
  create_table "applications", force: :cascade do |t|
    t.string "name"
    t.integer "index", null: false
    t.text "values"
  end
end
"#;

fn files() -> Vec<(String, String)> {
    spinel(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application.rb", "class Application < ApplicationRecord\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ])
}

#[test]
fn the_ddl_quotes_them() {
    let files = files();
    let seed = file(&files, "db/seed.sql");
    assert!(seed.contains("\"index\" INTEGER NOT NULL"), "{seed}");
    assert!(seed.contains("\"values\" TEXT"), "{seed}");
    assert!(seed.contains(" name TEXT"), "an ordinary column stays bare:\n{seed}");
}

#[test]
fn the_model_sql_quotes_them() {
    let files = files();
    let model = file(&files, "app/models/application.rb");
    // Rails' quoting in the projection: table and column always quoted,
    // the alias only where the name needs it.
    assert!(model.contains(r#"\"applications\".\"index\" AS \"index\""#), "{model}");
    assert!(model.contains(r#"\"applications\".\"name\" AS name"#), "{model}");
    assert!(model.contains(r#"SELECT id, name, \"index\", \"values\""#), "{model}");
    assert!(!model.contains(" index,"), "{model}");
}
