//! The Spinel side of `tests/typed_writer_cast.rs`: a typed column's
//! writer casts a value that is not a number or a String as Rails does
//! (`rails_answers_the_cast_table` there checks the answers against
//! activerecord 8.1.4), on the native binary as on CRuby. Needs a Spinel
//! compiler, so `#[ignore]`d like its sibling suites; CI's
//! `spinel-framework` job runs it with `--ignored` (`scripts/ci-plan.py`'s
//! `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.integer "count"
    t.float "ratio"
    t.decimal "price"
    t.boolean "active"
  end
end
"#;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_typed_writer_casts_a_non_numeric_value_on_spinel() {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_spinel_with_db(
            r#"
w = Widget.new
w.count = :many
raise "count :many: #{w.count.inspect}" unless w.count.nil?
w.count = []
raise "count []: #{w.count.inspect}" unless w.count.nil?
w.count = Object.new
raise "count Object: #{w.count.inspect}" unless w.count.nil?
w.count = "5"
raise "count \"5\": #{w.count.inspect}" unless w.count == 5
w.price = :many
raise "price :many: #{w.price.inspect}" unless w.price == 0.0
w.price = []
raise "price []: #{w.price.inspect}" unless w.price.nil?
w.price = "2.5"
raise "price \"2.5\": #{w.price.inspect}" unless w.price == 2.5
w.ratio = []
raise "ratio []: #{w.ratio.inspect}" unless w.ratio.nil?
raised = false
begin
  w.ratio = :many
rescue NoMethodError
  raised = true
end
raise "ratio :many did not raise" unless raised
w.active = :off
raise "active :off: #{w.active.inspect}" unless w.active == false
w.active = :""
raise "active :\"\": #{w.active.inspect}" unless w.active == true
puts "casts as Rails"
"#,
        )
        .assert_passes();
}
