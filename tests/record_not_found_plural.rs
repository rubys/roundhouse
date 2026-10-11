//! A multi-id finder miss must load its pluralizer itself: an app with
//! no pluralize helpers otherwise raises NameError instead of RecordNotFound.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :categories do |t|\n    t.string :name\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/category.rb", "class Category < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

const ASSERTIONS: &str = r#"
category = Category.create!(name: "present")
begin
  ActiveRecord::Relation.new(Category).find([category.id, 999])
  raise "find must raise when one of several ids is missing"
rescue ActiveRecord::RecordNotFound => error
  expected = "Couldn't find all Categories with 'id': (#{category.id}, 999) (found 1 results, but was looking for 2)."
  raise "wrong message: #{error.message}" unless error.message == expected
end
puts "multi-id message matches"
"#;

#[test]
fn a_multi_id_miss_uses_rails_wording_without_an_app_pluralize_helper() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(
        run.stdout.contains("multi-id message matches"),
        "{}",
        run.stdout
    );
}

#[test]
#[ignore = "requires the native Spinel compiler"]
fn a_multi_id_miss_uses_rails_wording_natively() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ASSERTIONS}"
    );
    let run = app().run_spinel(&script);
    run.assert_passes();
    assert!(
        run.stdout.contains("multi-id message matches"),
        "{}",
        run.stdout
    );
}
