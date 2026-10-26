use super::emit_and_run;

/// `exclusion: { in: … }` / `validates_exclusion_of`: the value must not
/// be one of the listed values (or, for a numeric range, not covered by
/// it). Rails' message is "is reserved". `nil` is reserved only when it's
/// listed: Rails' `ExclusionValidator` has no implicit `allow_nil`.
/// Answers below are Rails 8.1.4's for the same model and records.
const MODEL: &str = r#"class Widget < ApplicationRecord
  validates :domain, exclusion: { in: [""] }
  validates :status, exclusion: { in: %w[banned deleted] }
  validates :level, exclusion: { in: 1..3 }
  validates_exclusion_of :code, in: %w[root admin]
end
"#;

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "domain"
    t.string "status"
    t.float "level"
    t.string "code"
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
fn exclusion_rejects_reserved_values_like_rails() {
    app()
        .run_ruby(
            r#"def errors_for(attrs)
  w = Widget.new(**attrs)
  w.valid?
  w.errors.to_a
end
got = [
  errors_for({}),
  errors_for(domain: "", status: "banned", level: 2.0, code: "root"),
  errors_for(domain: "x", status: "live", level: 3.5, code: "bob"),
  errors_for(level: 3.0),
  errors_for(level: 1.5),
  errors_for(level: 0.5),
]
want = [
  [],
  ["Domain is reserved", "Status is reserved", "Level is reserved", "Code is reserved"],
  [],
  ["Level is reserved"],
  ["Level is reserved"],
  [],
]
raise "errors: #{got.inspect}" unless got == want
puts "exclusion passed"
"#,
        )
        .assert_passes();
}
