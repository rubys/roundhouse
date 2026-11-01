//! A concern's `included do` block may declare validations or
//! associations before a bare `private`. The marker only reaches the
//! methods defined after it, so those declarations keep their own
//! visibility, and the block is ingested as Rails runs it.
//!
//! A declaration after a marker, or a named marker, still refuses the
//! block: there, a generated method's visibility depends on the marker.
//!
//! Expected values are Rails 8.1.4's (activerecord 8.1.4, oracle
//! `rails-oracle.rb`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::project::BuildTarget;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"items\", force: :cascade do |t|\n    t.integer \"widget_id\"\n  end\nend\n";

const WIDGET: &str = "class Widget < ApplicationRecord\n  include Labelled\nend\n";

fn labelled(included_body: &str) -> String {
    format!("module Labelled\n  extend ActiveSupport::Concern\n\n  included do\n{included_body}  end\nend\n")
}

const DECLARATIONS_FIRST: &str = r##"    validates :name, presence: true

    def label
      "#{prefix}: #{name}"
    end

    private

    def prefix
      "widget"
    end
"##;

const ASSERTIONS: &str = r#"
got = [
  Widget.new(name: "x").label,
  Widget.new.valid?,
  Widget.public_method_defined?(:label),
  Widget.private_method_defined?(:prefix)
]
want = ["widget: x", false, true, true]
raise got.inspect unless got == want
puts "declarations before private passed"
"#;

fn ingest(included_body: &str) -> Result<roundhouse::App, roundhouse::ingest::IngestError> {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA.to_string()),
        ("app/models/widget.rb", WIDGET.to_string()),
        ("app/models/concerns/labelled.rb", labelled(included_body)),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect();
    roundhouse::ingest::ingest_app_from_tree(files)
}

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", WIDGET)
        .write("app/models/concerns/labelled.rb", &labelled(DECLARATIONS_FIRST))
}

#[test]
fn declarations_before_a_bare_private_are_ingested() {
    let (emitted, errors) = example().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let widget = std::fs::read_to_string(emitted.join("app/models/widget.rb")).expect("read widget.rb");
    assert!(widget.contains("def label") && widget.contains("def prefix"), "{widget}");
    assert!(widget.contains("private :prefix") && !widget.contains("private :label"), "{widget}");
}

#[test]
fn declarations_before_a_bare_private_run_on_ruby() {
    let run = example().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("declarations before private passed"), "{}", run.stdout);
}

/// After the marker, `validates` could generate a method the marker
/// reaches.
#[test]
fn a_declaration_after_a_bare_private_stays_rejected() {
    let err = ingest(
        "    def label\n      \"#{prefix}: #{name}\"\n    end\n\n    private\n\n    validates :name, presence: true\n\n    def prefix\n      \"widget\"\n    end\n",
    )
    .expect_err("a declaration after `private` stays rejected");
    assert!(format!("{err:?}").contains("not modeled"), "{err:?}");
}

/// A named marker reaches a method generated before it.
#[test]
fn a_named_private_of_a_generated_method_stays_rejected() {
    let err = ingest("    has_many :items\n    private :items\n")
        .expect_err("`private :items` of an association reader stays rejected");
    assert!(format!("{err:?}").contains("not modeled"), "{err:?}");
}
