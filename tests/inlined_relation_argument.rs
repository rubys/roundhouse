//! A relation materialized as a call argument keeps the call together:
//! the hydrate loop is a statement sequence, parenthesized in value
//! position so it reads as one grouped expression.

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
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.text "tags"
  end
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def tags
    render(json: Widget.all&.map(&:tags)&.flatten&.uniq || [])
  rescue StandardError
    render(json: [])
  end
end
"#;

#[test]
fn an_inlined_relation_in_an_argument_parses() {
    let files = spinel(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  serialize :tags, coder: JSON\nend\n"),
        ("app/controllers/widgets_controller.rb", CONTROLLER),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets/tags\", to: \"widgets#tags\"\nend\n"),
    ]);
    assert_parses(&files, "app/controllers/widgets_controller.rb");
}
