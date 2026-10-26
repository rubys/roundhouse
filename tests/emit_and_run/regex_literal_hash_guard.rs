use super::emit_and_run;

/// A pattern folded from single-quoted Strings holds `#@name`, `#$name`
/// or `#{name}` as text. The ruby emit wrote it into a `/…/` literal
/// unchanged, where Ruby reads each as an interpolation (`@name` is nil,
/// so `/\Ax#@name\z/` became `/\Ax\z/`). Answers are Rails 8.1.4's for
/// the same model and records: `b`'s `$` is an end anchor in the
/// pattern, so no value matches it.
#[test]
fn a_folded_hash_sequence_stays_text_like_rails() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"a\"\n    t.string \"b\"\n    t.string \"c\"\n  end\nend\n",
        )
        .write(
            "app/models/widget.rb",
            r##"class Widget < ApplicationRecord
  AT = 'x#@name'
  DOLLAR = 'x#$name'
  BRACE = 'x#{name}'
  validates :a, format: { with: /\A#{AT}\z/ }
  validates :b, format: { with: /\A#{DOLLAR}\z/ }
  validates :c, format: { with: /\A#{BRACE}\z/ }
end
"##,
        )
        .run_ruby(
            r##"rows = [{ a: 'x#@name', b: 'x#$name', c: 'x#{name}' }, { a: 'x', b: 'x', c: 'x' }]
got = rows.map do |attrs|
  w = Widget.new
  attrs.each { |k, v| w.public_send(:"#{k}=", v) }
  w.valid?
  w.errors.to_a
end
want = [["B is invalid"], ["A is invalid", "B is invalid", "C is invalid"]]
raise "widget errors: #{got.inspect}" unless got == want
puts "regex literal hash guard passed"
"##,
        )
        .assert_passes();
}
