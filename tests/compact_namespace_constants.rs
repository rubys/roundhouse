//! A constant of a top-level compact class (`class Feeds::Catalog`) whose
//! namespace no file declares: Zeitwerk defines `Feeds` from the
//! directory, and the class only loads where `Feeds` exists, so the path
//! resolves and the constant folds where a body reads it. Before, the
//! unknown `Feeds` stopped the lookup, and `inclusion: { in:
//! Feeds::Catalog::KINDS }` was a "validation option not lowered" gap.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

#[test]
fn a_compact_class_constant_folds_without_its_namespace_declared() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"kind\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/lib/feeds/catalog.rb", "class Feeds::Catalog\n  KINDS = %w[news blog].freeze\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  validates :kind, inclusion: { in: Feeds::Catalog::KINDS }\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(
            r#"raise "news rejected" unless Widget.new(kind: "news").valid?
raise "other accepted" if Widget.new(kind: "other").valid?
puts "PASS"
"#,
        )
        .assert_passes();
}

/// Inside another body the compact path resolves lexically, so nothing
/// is assumed about the enclosing names there (`module Wrapper; class
/// Catalog::Kinds` reopens a top-level `Catalog`, not `Wrapper::Catalog`).
#[test]
fn a_nested_compact_path_assumes_no_namespace() {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"kind\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/lib/wrapper.rb", "module Wrapper\n  class Catalog::Kinds\n    LIST = %w[news blog].freeze\n  end\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  validates :kind, inclusion: { in: Wrapper::Catalog::Kinds::LIST }\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let widget = app.models.iter().find(|m| m.name.0.as_str() == "Widget").expect("Widget");
    assert!(
        !format!("{:?}", widget.validations().collect::<Vec<_>>()).contains("\"news\""),
        "a nested compact path must not fold through an assumed namespace: {:?}",
        widget.validations().collect::<Vec<_>>()
    );
}
