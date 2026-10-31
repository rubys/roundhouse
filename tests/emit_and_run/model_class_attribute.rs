//! A model's `class_attribute`, written by a class method its own body
//! calls as it loads. A subclass reads its parent's value until it
//! writes its own.

use super::emit_and_run;

const SCHEMA: &str = "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"title\"\n    t.string \"type\"\n  end\nend\n";

const NOTE: &str = r#"class Note < ApplicationRecord
  class_attribute :strategies, default: {}

  def self.custom_strategy(field, with:)
    self.strategies = strategies.merge(field => with)
  end

  custom_strategy :title, with: proc { |r| "redacted-#{r.id}" }

  def redact_title = self.class.strategies.fetch(:title).call(self)
end
"#;

const NOTE_TEST: &str = r#"require "test_helper"

class NoteTest < ActiveSupport::TestCase
  test "the class body's macro call writes the attribute" do
    note = Note.create!(title: "secret")
    assert_equal [:title], Note.strategies.keys
    assert_equal "redacted-#{note.id}", note.redact_title
    assert_equal true, Note.strategies?
  end

  test "an instance write is the instance's own" do
    note = Note.create!(title: "secret")
    note.strategies = {}
    assert_equal({}, note.strategies)
    assert_equal [:title], Note.strategies.keys
  end

  test "a subclass reads its parent's value until it writes its own" do
    assert_equal [:title], SecretNote.strategies.keys
    SecretNote.custom_strategy :body, with: proc { |r| "hidden" }
    assert_equal [:title, :body], SecretNote.strategies.keys
    assert_equal [:title], Note.strategies.keys
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/note.rb", NOTE)
        .write("app/models/secret_note.rb", "class SecretNote < Note\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("test/models/note_test.rb", NOTE_TEST)
}

#[test]
fn a_models_class_attribute_written_by_its_class_body_runs() {
    app().run_test("test/models/note_test.rb").assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_models_class_attribute_written_by_its_class_body_runs_on_spinel() {
    let run = app().run_spinel_with_db(
        "note = Note.create!(title: \"secret\")\np Note.strategies.keys\nputs note.redact_title\np SecretNote.strategies.keys\n",
    );
    run.assert_passes();
    assert_eq!(run.stdout, "[:title]\nredacted-1\n[:title]\n", "{}", run.stderr);
}
