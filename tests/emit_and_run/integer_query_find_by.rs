use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.integer :number\n    t.string :label\n  end\nend\n")
        .write("app/models/widget.rb", r#"class Widget < ApplicationRecord
  def self.lookup(conditions)
    find_by(conditions)
  end
end
"#)
}

const ASSERTIONS: &str = r#"
Db.exec("INSERT INTO widgets (id, number, label) VALUES (1, 7, 'present'), (2, NULL, 'unset')")
record = Widget.lookup(number: nil)
raise "model Hash finder misses NULL" unless record && record.id == 2
record = Widget.find_by(number: nil)
raise "class finder misses NULL" unless record && record.id == 2
record = Widget.lookup(number: [7, 9])
raise "model Hash finder misses IN values" unless record && record.id == 1
record = Widget.lookup({"number" => nil})
raise "string Hash keys miss NULL" unless record && record.id == 2
raise "empty IN matches rows" if Widget.lookup(number: [])
record = Widget.lookup(number: "7")
raise "numeric string equality regresses" unless record && record.id == 1
record = Widget.lookup({})
raise "empty Hash misses rows" unless record
record = Widget.find_by(number: 7)
raise "scalar finder regresses" unless record && record.id == 1
raise "compound conditions ignored" if Widget.lookup(number: nil, label: 'present')
raise "missing row returned" if Widget.lookup(number: [9, 10])
record = Widget.find_by!(number: nil)
raise "bang finder misses NULL" unless record.id == 2
begin
  Widget.find_by!(number: 99)
  raise "missing bang finder does not raise"
rescue ActiveRecord::RecordNotFound
end
puts "Model Hash finder predicates passed"
"#;

#[test]
fn model_hash_finder_predicates_run_on_ruby() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn model_hash_finder_predicates_run_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ASSERTIONS}"
    );
    app().run_spinel(&script).assert_passes();
}
