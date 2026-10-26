use super::emit_and_run;

/// `validates …, on: :create` / `on: :update` / `on: [:create, :update]`.
/// Rails runs a validation in the context `valid?` / `save` use: `:create`
/// for a new record, `:update` for a persisted one, so an `on: :create`
/// check is skipped once the record is saved. Combined here with an
/// `unless:` condition on the same call. Answers below are Rails 8.1.4's
/// for the same model and records.
const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :uri, presence: true, on: :create
  validates :note, presence: true, on: :update
  validates :code, presence: true, unless: :local?, on: :create
  validates :uri, length: { maximum: 5 }, on: [:create, :update]
end
"#;

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "uri"
    t.string "note"
    t.string "code"
    t.boolean "local", default: false
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", MODEL)
}

#[test]
fn on_context_validations_run_when_rails_runs_them() {
    app()
        .run_ruby(
            r#"fresh = Widget.new
fresh.valid?
local = Widget.new(local: true)
local.valid?
saved = Widget.create!(uri: "a", code: "c")
saved.uri = nil
saved.code = nil
saved.valid?
blank_update = saved.errors.to_a
saved.note = "n"
saved.uri = "toolong"
saved.valid?
got = [fresh.errors.to_a, local.errors.to_a, blank_update, saved.errors.to_a]
want = [
  ["Uri can't be blank", "Code can't be blank"],
  ["Uri can't be blank"],
  ["Note can't be blank"],
  ["Uri is too long (maximum is 5 characters)"],
]
raise "errors: #{got.inspect}" unless got == want
puts "on: contexts passed"
"#,
        )
        .assert_passes();
}
