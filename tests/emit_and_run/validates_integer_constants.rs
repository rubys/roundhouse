use super::emit_and_run;

/// A `length:` or `numericality:` bound given as an integer constant of
/// the model (`maximum: NAME_LIMIT`), which Rails reads when the class
/// body runs. The ingest only read integer literals, so the option was a
/// reported gap and the whole call was refused. Answers below are Rails
/// 8.1.4's for the same model and records.
const MODEL: &str = r#"class Widget < ApplicationRecord
  NAME_MIN = 2
  NAME_LIMIT = 5
  MAX_SCORE = 100

  validates :name, length: { minimum: NAME_MIN, maximum: NAME_LIMIT }
  validates :score, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: MAX_SCORE }
  validates_numericality_of :rank, greater_than: NAME_MIN
end
"#;

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.integer "score"
    t.integer "rank"
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
fn constant_bounds_validate_like_rails() {
    app()
        .run_ruby(
            r#"def errors_for(name, score, rank)
  w = Widget.new(name: name, score: score, rank: rank)
  w.valid?
  w.errors.to_a
end
got = [errors_for("abc", 50, 3), errors_for("a", 50, 3), errors_for("abcdef", 101, 2)]
want = [
  [],
  ["Name is too short (minimum is 2 characters)"],
  ["Name is too long (maximum is 5 characters)", "Score must be less than or equal to 100", "Rank must be greater than 2"],
]
raise "errors: #{got.inspect}" unless got == want
puts "integer constant bounds passed"
"#,
        )
        .assert_passes();
}
