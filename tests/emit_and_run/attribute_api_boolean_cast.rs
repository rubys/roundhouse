use super::emit_and_run;

/// `attribute :name, :boolean` writes through Rails 8.1.4's
/// `ActiveModel::Type::Boolean#cast`: nil and `""` read back as nil,
/// `"0"`, `"f"`, `"F"`, `"false"`, `"FALSE"`, `"off"`, `"OFF"` (and
/// `false`, `0`, `:off`, whose `to_s` is one of them) as `false`, and
/// anything else as `true`. The writer used to turn nil and `""` into
/// `false`, and `"off"`/`"F"`/`"FALSE"` into `true`.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"signups\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/signup.rb", "class Signup < ApplicationRecord\n  attribute :consent, :boolean\nend\n")
}

#[test]
fn a_boolean_attribute_casts_like_rails() {
    let run = app().run_ruby(
        r##"values = [nil, "", "0", "f", "F", "false", "FALSE", "off", "OFF", "1", "t", "true", "yes", "on", "  ", 0, 1, true, false, :off, "0.0"]
got = values.map do |v|
  s = Signup.new
  s.consent = v
  s.consent
end
want = [nil, nil, false, false, false, false, false, false, false, true, true, true, true, true, true, false, true, true, false, false, true]
raise "casts: #{values.zip(got).inspect}" unless got == want
raise "unset: #{Signup.new.consent.inspect}" unless Signup.new.consent.nil?
puts "boolean cast passed"
"##,
    );
    run.assert_passes();
    assert!(run.stdout.contains("boolean cast passed"), "{}", run.stdout);
}
