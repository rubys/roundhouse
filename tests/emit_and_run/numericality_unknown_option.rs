use super::emit_and_run;

/// A `numericality:` key Rails' `NumericalityValidator` does not read
/// (`greater_than_or_equal:`, where the bound is
/// `greater_than_or_equal_to:`) is ignored by Rails: the rule checks
/// only that the value is a number. It used to refuse the whole call.
/// Answers are Rails 8.1.4's for the same model and records.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"collections\", force: :cascade do |t|\n    t.integer \"n\"\n  end\nend\n",
        )
        .write(
            "app/models/collection.rb",
            "class Collection < ApplicationRecord\n  validates :n, presence: true, numericality: { greater_than_or_equal: 0 }\nend\n",
        )
}

#[test]
fn an_option_rails_does_not_read_is_ignored_as_rails_ignores_it() {
    let run = app().run_ruby(
        r##"got = [-5, 0, 3, "abc", nil, 1.5].map do |v|
  c = Collection.new
  c.n = v
  c.valid?
  c.errors.to_a
end
want = [[], [], [], ["N is not a number"], ["N can't be blank", "N is not a number"], []]
raise "collection errors: #{got.inspect}" unless got == want
puts "ignored option passed"
"##,
    );
    run.assert_passes();
    assert!(run.stdout.contains("ignored option passed"), "{}", run.stdout);
}
