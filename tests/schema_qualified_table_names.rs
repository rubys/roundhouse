//! `create_table "public.widgets"` names the table `widgets`.
//!
//! When `schema_search_path` lists more than one schema, the Postgres
//! dumper qualifies every table in `schema.rb`. Nothing else in the app
//! uses the qualifier, so the model must meet its table by the bare name.

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
  create_schema "shared_extensions"
  enable_extension "shared_extensions.pgcrypto"

  create_table "public.widgets", force: :cascade do |t|
    t.string "name", null: false
    t.timestamps
    t.index ["name"], name: "index_widgets_on_name"
  end
end
"#;

fn files() -> Vec<(String, String)> {
    spinel(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ])
}

#[test]
fn the_seed_creates_the_bare_table() {
    let files = files();
    let seed = file(&files, "db/seed.sql");
    assert!(seed.contains("CREATE TABLE IF NOT EXISTS widgets ("), "{seed}");
    assert!(!seed.contains("public."), "{seed}");
    assert!(seed.contains("ON widgets (name)"), "its index names the bare table too:\n{seed}");
}

#[test]
fn the_model_keeps_its_columns() {
    let files = files();
    let model = file(&files, "app/models/widget.rb");
    assert!(model.contains("def name"), "{model}");
    assert!(model.contains("FROM widgets"), "{model}");
}
