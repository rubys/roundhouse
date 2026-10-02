//! `rails generate model` writes a test class with no test methods.
//! The Spinel reshape refused the whole app on one such file, and
//! `--allow-unsupported` could not help, because the refusal was an
//! `Err` and not a diagnostic (NEXUS_BUGS.md B5).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::diagnostic::{Diagnostic, Severity};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\", null: false\n  end\nend\n";
const EMPTY_POST_TEST: &str =
    "require \"test_helper\"\n\nclass PostTest < ActiveSupport::TestCase\nend\n";

/// The spinel tree for a small posts app plus `files`, with the
/// diagnostics that the emit pushed.
fn spinel(files: &[(&str, &str)]) -> (Result<Vec<(String, String)>, String>, Vec<Diagnostic>) {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (path, content) in [
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .chain(files.iter().copied())
    {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::diagnostics::scope(|| target_files(&app, Path::new("."), BuildTarget::Spinel))
}

#[test]
fn an_empty_test_class_does_not_stop_the_spinel_emit() {
    let (files, diags) = spinel(&[("test/models/post_test.rb", EMPTY_POST_TEST)]);

    let files = files.unwrap_or_else(|e| panic!("{e}"));
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    assert!(!paths.iter().any(|p| p.contains("post_test")), "{paths:#?}");
    let makefile = &files.iter().find(|(p, _)| p == "Makefile").expect("Makefile").1;
    assert!(!makefile.contains("post_test"), "{makefile}");

    let errors: Vec<&str> =
        diags.iter().filter(|d| d.severity == Severity::Error).map(|d| d.message.as_str()).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let warnings: Vec<&str> = diags
        .iter()
        .filter(|d| d.severity == Severity::Warning && d.message.contains("post_test.rb"))
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(warnings.len(), 1, "{diags:#?}");
}
