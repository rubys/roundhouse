//! A model method keeps its `*rest` and `&block` parameters in the
//! emitted `def`, and the `.rbs` sidecar declares the rest.
//!
//! `ingest::model::ingest_method` recorded requireds, optionals,
//! keywords and `**kwrest`, and let `*rest` and `&block` fall through,
//! so `def tagged(*labels)` on an `ApplicationRecord` subclass came out
//! as `def tagged` with a body still reading `labels` — an ArgumentError
//! at every call site — while the same method on a plain class under
//! `app/models` kept its splat (#208).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

const WIDGET: &str = r#"class Widget < ApplicationRecord
  def tagged(*labels)
    labels.join(",")
  end

  def pair(first, *rest)
    [first, rest.size]
  end

  def each_label(&blk)
    [name].each(&blk)
  end

  def both(*args, **opts)
    [args, opts]
  end
end
"#;

const REPORT: &str = r#"class Report
  def tagged(*labels)
    labels.join(",")
  end
end
"#;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";

/// The emitted tree for `target`: the two base classes, a model with
/// every rest/block shape, and a plain class with a splat for contrast.
fn emitted(target: BuildTarget) -> Vec<(String, String)> {
    let files = [
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/models/widget.rb", WIDGET),
        ("app/models/report.rb", REPORT),
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(path, content)| (PathBuf::from(path), content.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), target).expect("target files")
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not emitted"))
        .1
}

fn assert_parses(source: &str, path: &str) {
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

fn assert_defs_keep_rest_and_block(files: &[(String, String)]) {
    let widget = file(files, "app/models/widget.rb");
    assert_parses(widget, "app/models/widget.rb");
    assert!(widget.contains("def tagged(*labels)"), "{widget}");
    assert!(widget.contains("def pair(first, *rest)"), "{widget}");
    assert!(widget.contains("def each_label(&blk)"), "{widget}");
    assert!(widget.contains("def both(*args, **opts)"), "{widget}");

    let report = file(files, "app/models/report.rb");
    assert!(report.contains("def tagged(*labels)"), "{report}");
}

#[test]
fn the_spinel_def_keeps_rest_and_block_params() {
    assert_defs_keep_rest_and_block(&emitted(BuildTarget::Spinel));
}

#[test]
fn the_ruby_def_keeps_rest_and_block_params() {
    assert_defs_keep_rest_and_block(&emitted(BuildTarget::Ruby));
}

#[test]
fn the_sidecar_declares_the_rest_params() {
    let files = emitted(BuildTarget::Spinel);
    let rbs = file(&files, "app/models/widget.rbs");
    assert!(rbs.contains("def tagged: (*untyped labels)"), "{rbs}");
    assert!(rbs.contains("def pair: (untyped first, *untyped rest)"), "{rbs}");
    assert!(rbs.contains("def both: (*untyped args, **untyped opts)"), "{rbs}");
}
