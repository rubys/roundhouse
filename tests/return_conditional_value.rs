//! `return a ? b : c if cond` keeps its conditional value together:
//! `return if … else … end` re-parses as a modifier on a bare `return`.

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

const RECIPIENT: &str = r#"class Recipient
  def self.email(data)
    recipients = Array(data["messages"]).uniq
    return recipients.size == 1 ? recipients.first : recipients.last if recipients.any?

    data["fallback"]
  end
end
"#;

#[test]
fn a_conditional_return_value_parses() {
    let files = spinel(&[
        ("app/services/recipient.rb", RECIPIENT),
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]);
    assert_parses(&files, "app/models/recipient.rb");
    assert!(file(&files, "app/models/recipient.rb").contains("return (if "));
}
