//! `Associations::Preloader` resolves at check and preloads on the
//! ruby target. A `scope:` keyword is not rewritten away; MRI raises
//! unknown-keyword `ArgumentError`.

use super::emit_and_run;

#[test]
fn associations_preloader_loads_and_refuses_a_scope() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
  create_table "parts", force: :cascade do |t|
    t.integer "widget_id"
    t.string "label"
  end
end
"#,
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :parts\nend\n")
        .write("app/models/part.rb", "class Part < ApplicationRecord\n  belongs_to :widget\nend\n")
        .write(
            "app/models/widget_digest.rb",
            r#"class WidgetDigest
  def self.labels(records)
    ActiveRecord::Associations::Preloader.new(records: records, associations: :parts).call
    Part.delete_all
    records.map { |widget| widget.parts.size }.join(",")
  end

  def self.scoped(records)
    ActiveRecord::Associations::Preloader.new(records: records, associations: :parts, scope: Part.where(label: "a")).call
  end
end
"#,
        )
        .run_ruby(
            r#"
alpha = Widget.create!(name: "alpha")
part = Part.create!(widget: alpha, label: "a1")
raise "fk #{part.widget_id.inspect}" unless part.widget_id == alpha.id
got = WidgetDigest.labels(Widget.order(:id).to_a)
raise "labels #{got.inspect}" unless got == "1"
begin
  WidgetDigest.scoped([])
  raise "scope was accepted"
rescue ArgumentError => error
  raise error.message unless error.message.include?("scope")
end
"#,
        )
        .assert_passes();
}
