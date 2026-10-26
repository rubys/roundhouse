use super::emit_and_run;

/// `format: { with: CONST }` where CONST interpolates String and Integer
/// constants (see tests/regex_string_interpolation.rs). Answers are Rails
/// 8.1.4's for the same model and records: the `·` separator matches, the
/// `i` flag applies to the whole spliced pattern, and the Integer sets the
/// repetition count.
#[test]
fn a_regex_built_from_string_constants_validates_like_rails() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.string \"code\"\n    t.string \"sized\"\n  end\nend\n",
        )
        .write(
            "app/models/widget.rb",
            r##"class Widget < ApplicationRecord
  SEPARATORS = "_·"
  FIRST = "x[[:word:]#{SEPARATORS}]*".freeze
  NAME_PAT = "#{FIRST}|[[:digit:]]+".freeze
  NAME_RE = /\A(#{NAME_PAT})\z/i
  CODE_FRAGMENT = '[a-z0-9_]{2,}'
  CODE_RE = /\A#{CODE_FRAGMENT}\z/
  SIZE = 3
  SIZED_RE = /\A[a-z]{#{SIZE}}\z/

  validates :name, format: { with: NAME_RE }
  validates :code, format: { with: CODE_RE }
  validates :sized, format: { with: SIZED_RE }
end
"##,
        )
        .run_ruby(
            r#"rows = [["x1·a", "ab", "abc"], ["X1·a", "a", "ab"], ["y1", "AB", "abcd"], ["123", "a_1", "xyz"]]
got = rows.map do |name, code, sized|
  w = Widget.new(name: name, code: code, sized: sized)
  w.valid?
  w.errors.to_a
end
want = [[], ["Code is invalid", "Sized is invalid"], ["Name is invalid", "Code is invalid", "Sized is invalid"], []]
raise "errors: #{got.inspect}" unless got == want
puts "interpolated regex constants passed"
"#,
        )
        .assert_passes();
}
