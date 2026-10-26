use super::emit_and_run;

/// Numericality bounds given as Integer arithmetic over literals and the
/// model's own Integer constants (`(2**31) - 1`, `2**63`, `4 * (2 + 3)`,
/// `LEVEL_MAX - 1`), and the `in:` range form (`in: (-LIMIT..LIMIT)`,
/// `in: 1...3`). Each was a reported gap that refused the whole call.
/// Answers are Rails 8.1.4's for the same model and records, including a
/// value at each exact limit and the `2**63` bound's message.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"roles\", force: :cascade do |t|\n    t.integer \"position\"\n    t.bigint \"sign_count\"\n    t.integer \"level\"\n    t.integer \"tier\"\n  end\nend\n",
        )
        .write(
            "app/models/role.rb",
            r#"class Role < ApplicationRecord
  POSITION_LIMIT = (2**31) - 1
  SIGN_COUNT_LIMIT = (2**63)
  LEVEL_MAX = 4 * (2 + 3)

  validates :position, numericality: { in: (-POSITION_LIMIT..POSITION_LIMIT) }
  validates :sign_count, numericality: { only_integer: true, greater_than_or_equal_to: 0, less_than: SIGN_COUNT_LIMIT }
  validates :level, numericality: { less_than_or_equal_to: LEVEL_MAX - 1 }
  validates :tier, numericality: { in: 1...3 }
end
"#,
        )
}

#[test]
fn arithmetic_bounds_validate_like_rails() {
    app()
        .run_ruby(
            r#"rows = [
  [2147483647, 0, 19, 1],
  [2147483648, 2**63 - 1, 20, 3],
  [-2147483647, -1, -5, 2],
  [-2147483648, 2**63, 0, 2],
]
got = rows.map do |position, sign_count, level, tier|
  r = Role.new
  r.position = position
  r.sign_count = sign_count
  r.level = level
  r.tier = tier
  r.valid?
  r.errors.to_a
end
want = [
  [],
  ["Position must be in -2147483647..2147483647", "Level must be less than or equal to 19", "Tier must be in 1...3"],
  ["Sign count must be greater than or equal to 0"],
  ["Position must be in -2147483647..2147483647", "Sign count must be less than 9223372036854775808"],
]
raise "errors: #{got.inspect}" unless got == want
puts "arithmetic bounds passed"
"#,
        )
        .assert_passes();
}

/// The `in:` check is shared IR: the strict targets emit it with no error.
#[test]
fn strict_targets_emit_the_bounds_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}
