//! A nested class spliced into its parent's file (#151) is declared in
//! the parent's `.rbs` sidecar.
//!
//! The child lost its own file and, with it, the sidecar that file
//! carried; nothing was added to the parent's. The `.rb` kept every
//! nested class while the typed view of the tree declared none of them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{BuildTarget, target_files};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";
const ROUTES: &str =
    "Rails.application.routes.draw do\n  get \"/widgets/:id\", to: \"widgets#show\"\nend\n";

/// The spinel tree for a small app: the base classes, a `widgets`
/// table, one route, plus `files`.
fn spinel(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        APPLICATION_RECORD.as_bytes().to_vec(),
    );
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        APPLICATION_CONTROLLER.as_bytes().to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/widget.rb"),
        b"class Widget < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        ROUTES.as_bytes().to_vec(),
    );
    for (path, content) in files {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files")
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| {
            panic!(
                "{path} not emitted: {:?}",
                files.iter().map(|(p, _)| p).collect::<Vec<_>>()
            )
        })
        .1
}

fn assert_parses(files: &[(String, String)], path: &str) {
    let source = file(files, path);
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(
        errors.is_empty(),
        "{path} does not parse: {errors:?}\n{source}"
    );
}

fn assert_rbs_parses(files: &[(String, String)], path: &str) {
    let source = file(files, path);
    if let Err(e) = ruby_rbs::node::parse(source) {
        panic!("{path} is not valid RBS: {e}\n{source}");
    }
}

const REPORT: &str = r#"class Report
  class Failure < StandardError
    attr_reader :hint

    def initialize(message, hint = nil)
      super(message)
      @hint = hint
    end
  end

  def self.render(widget)
    raise Failure.new("no widget", "check the id") if widget.nil?
    "report for #{widget.name}"
  end
end
"#;

const CONTROLLER: &str = r##"class WidgetsController < ApplicationController
  def show
    widget = Widget.find_by(id: params[:id])
    render plain: Report.render(widget)
  rescue Report::Failure => e
    render plain: "#{e.message} (#{e.hint})", status: 422
  end
end
"##;

#[test]
fn a_nested_class_is_declared_in_its_parents_sidecar() {
    let files = spinel(&[
        ("app/models/report.rb", REPORT),
        ("app/controllers/widgets_controller.rb", CONTROLLER),
    ]);
    // The `.rb` half of #151 still holds, and still parses.
    assert_parses(&files, "app/models/report.rb");
    assert!(file(&files, "app/models/report.rb").contains("class Failure < StandardError"));
    assert!(
        files
            .iter()
            .all(|(p, _)| !p.ends_with("report/failure.rb") && !p.ends_with("report/failure.rbs")),
        "the nested class keeps no file of its own: {:?}",
        files.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );

    let rbs = file(&files, "app/models/report.rbs");
    assert_rbs_parses(&files, "app/models/report.rbs");
    // Nested inside the parent's declaration, with the child's members.
    assert!(
        rbs.starts_with("class Report\n  class Failure < StandardError\n"),
        "{rbs}"
    );
    assert!(rbs.contains("\n    attr_reader hint: "), "{rbs}");
    assert!(rbs.contains("\n    def initialize: ("), "{rbs}");
    // The parent's own member is still there, after the child.
    let child = rbs.find("class Failure").expect("the child is declared");
    let own = rbs
        .find("def self.render:")
        .expect("the parent's method is declared");
    assert!(
        child < own,
        "the child comes before the parent's body:\n{rbs}"
    );
}

#[test]
fn a_class_nested_two_deep_is_declared_in_its_parents_sidecar() {
    // The wrapper is removed by count, so a parent with two segments
    // is where an off-by-one would show.
    let report = REPORT
        .replace('\n', "\n  ")
        .replace("class Report", "module Billing\n  class Report");
    let report = format!("{}\nend\n", report.trim_end());
    let controller = CONTROLLER.replace("Report", "Billing::Report");
    let files = spinel(&[
        ("app/models/billing/report.rb", &report),
        ("app/controllers/widgets_controller.rb", &controller),
    ]);
    assert_parses(&files, "app/models/billing/report.rb");
    let rbs = file(&files, "app/models/billing/report.rbs");
    assert_rbs_parses(&files, "app/models/billing/report.rbs");
    assert!(
        rbs.starts_with("module Billing\n  class Report\n    class Failure < StandardError\n"),
        "{rbs}"
    );
    assert!(rbs.contains("\n      attr_reader hint: "), "{rbs}");
    assert!(rbs.contains("\n    def self.render:"), "{rbs}");
}
