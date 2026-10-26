use super::emit_and_run;

/// `allow_blank:` skips a value Rails' `blank?` calls blank: a String of nothing but Unicode whitespace
/// (`/\A[[:space:]]*\z/`: U+3000 and U+00A0 count, U+200B does not). The
/// ruby family's skip tested `to_s.strip.empty?`, which keeps U+3000 and
/// U+00A0, so those were checked and rejected. Numericality shares the
/// skip (`allow_blank_cond`); a format rule on a String column shows it,
/// as a numericality one there is the reported #1158 gap. Answers are
/// Rails 8.1.4's for the same model and values.
#[test]
fn allow_blank_skips_unicode_whitespace_like_rails() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"gadgets\", force: :cascade do |t|\n    t.string \"code\"\n  end\nend\n",
        )
        .write(
            "app/models/gadget.rb",
            "class Gadget < ApplicationRecord\n  validates :code, format: { with: /\\A[A-Z]+\\z/ }, allow_blank: true\nend\n",
        )
        .run_ruby(
            r##"values = ["　", " ", " \t", "​", "x"]
got = values.map do |v|
  g = Gadget.new
  g.code = v
  g.valid?
  g.errors.to_a
end
bad = ["Code is invalid"]
want = [[], [], [], bad, bad]
raise "errors: #{got.inspect}" unless got == want
puts "unicode blank passed"
"##,
        )
        .assert_passes();
}
