//! A parameter every call site passes `nil` to is seeded `untyped`, not
//! `nil`. Spinel refuses a `nil`-typed seed ("param has unsupported type
//! nil"), and nothing about the method says the argument must be nil.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

/// The spinel tree for a small app: the two base classes plus `files`.
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
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

const SEARCH: &str = r##"class Search
  def self.run(scope, query)
    query.nil? ? scope : "#{scope}:#{query}"
  end

  def self.build(name:, options: nil)
    options.nil? ? name : "#{name} #{options}"
  end
end
"##;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def index
    @q = Search.run("all", nil)
    @b = Search.build(name: "a", options: nil)
  end
end
"#;

#[test]
fn a_nil_only_parameter_is_seeded_untyped() {
    let files = spinel(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/services/search.rb", SEARCH),
        ("app/controllers/widgets_controller.rb", CONTROLLER),
        ("app/views/widgets/index.json.jbuilder", "json.q @q\njson.b @b\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  root \"widgets#index\"\nend\n"),
    ]);
    let rbs = file(&files, "app/models/search.rbs");
    assert!(!rbs.contains(" nil query") && !rbs.contains(": nil"), "a nil-only parameter is seeded nil:\n{rbs}");
    assert!(rbs.contains("untyped query"), "{rbs}");
    assert!(rbs.contains("String scope"), "a parameter with a concrete type keeps it:\n{rbs}");
}
