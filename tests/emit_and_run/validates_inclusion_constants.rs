use super::emit_and_run;

/// `inclusion: { in: NAME }` / `validates_inclusion_of … in: NAME` with
/// `NAME` a list constant of the model (`%w[…].freeze`, `%i[…]`, an
/// array literal, or another list constant). Ingest only read an array
/// literal written in place, so a constant list was a reported gap and the
/// call was refused. The list keeps each element's kind: Rails'
/// `include?` tells a Symbol from a String, so a `%i[…]` list never
/// matches a string column's value. Answers below are Rails 8.1.4's for
/// the same model and records.
const MODEL: &str = r#"class Widget < ApplicationRecord
  MODES = %w(open approved none).freeze
  KINDS = %i[a b]
  ALIASED = MODES
  LEVELS = [1, 2, 3].freeze

  validates :mode, inclusion: { in: MODES }
  validates :kind, inclusion: { in: KINDS }
  validates :other, inclusion: { in: ALIASED }
  validates_inclusion_of :x, in: MODES
  validates :level, inclusion: { in: LEVELS }
end
"#;

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "mode"
    t.string "kind"
    t.string "other"
    t.string "x"
    t.integer "level"
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
fn inclusion_in_a_list_constant_matches_rails() {
    app()
        .run_ruby(
            r#"def errors_for(**attrs)
  w = Widget.new(**attrs)
  w.valid?
  w.errors.to_a
end
got = [
  errors_for(mode: "open", kind: :a, other: "none", x: "open", level: 1),
  errors_for(mode: "nope", kind: "a", other: "open", x: "approved", level: 3),
  errors_for(mode: "none", kind: :b, other: "x", x: "zzz", level: 7),
]
want = [
  ["Kind is not included in the list"],
  ["Mode is not included in the list", "Kind is not included in the list"],
  ["Kind is not included in the list", "Other is not included in the list", "X is not included in the list", "Level is not included in the list"],
]
raise "errors: #{got.inspect}" unless got == want
puts "inclusion constants passed"
"#,
        )
        .assert_passes();
}
