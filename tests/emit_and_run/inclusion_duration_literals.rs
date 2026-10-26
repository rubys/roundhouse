use super::emit_and_run;

/// `inclusion: { in: [...] }` over Duration literals (`1.week.seconds`,
/// `30.minutes`), in a list constant or inline. A Duration equals its
/// Integer seconds both ways, so Rails answers `include?` as the Integer
/// list does; the list now folds to those Integers. It was a reported gap
/// that refused the call. Answers are Rails 8.1.4's for the same model and
/// records.
#[test]
fn duration_literals_fold_to_their_seconds_like_rails() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"policies\", force: :cascade do |t|\n    t.integer \"min_age\"\n    t.integer \"window\"\n  end\nend\n",
        )
        .write(
            "app/models/policy.rb",
            r#"class Policy < ApplicationRecord
  ALLOWED = [1.week.seconds, 2.weeks.seconds, 1.month.seconds, 6.months.seconds, 1.year.seconds, 2.years.seconds].freeze

  validates :min_age, inclusion: { in: ALLOWED }
  validates :window, inclusion: { in: [30.minutes, 1.hour, 1.day.seconds] }
end
"#,
        )
        .run_ruby(
            r##"rows = [[604800, 1800], [15778476, 3600], [63113904, 86400], [86400, 60], [2629746, 1800]]
got = rows.map do |m, w|
  x = Policy.new
  x.min_age = m
  x.window = w
  x.valid?
  x.errors.to_a
end
want = [[], [], [], ["Min age is not included in the list", "Window is not included in the list"], []]
raise "errors: #{got.inspect}" unless got == want
puts "duration inclusion passed"
"##,
        )
        .assert_passes();
}
