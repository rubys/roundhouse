//! An enum label is mapped through the enum of the model the query is
//! on, not through whatever enum some model declares on a column of the
//! same name.
//!
//! The hand-written label rewrite was keyed by column name alone, and a
//! column that two models map differently (`Widget` and `Gadget` both
//! declare `enum :role`, with different labels) was dropped from it.
//! Every chained `where(role: …)`, `where.not(role: …)` and constant
//! list on either model then reached the SQL as Symbols, so the query
//! silently answered the wrong rows. Rails casts each value through the
//! enum type of the relation's own model.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn enum_labels_follow_the_relations_model_when_two_models_share_a_column() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.integer "role", default: 0, null: false
    t.boolean "active", default: true
  end

  create_table "gadgets", force: :cascade do |t|
    t.string "label"
    t.integer "role", default: 0, null: false
  end
end
"#,
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  STAFF = %i[member admin].freeze\n\n  enum :role, %i[member admin owner]\n\n  def self.staff\n    where(active: true).where(role: STAFF)\n  end\nend\n",
        )
        .write("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  enum :role, %i[guest owner]\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  ROLES = %i[admin owner].freeze
  private_constant :ROLES

  def index
    render json: {
      listed: Widget.where(role: ROLES, active: true).order(:name).pluck(:name),
      others: Widget.where(active: true).where.not(role: :admin).order(:name).pluck(:name),
      chained: Widget.where(active: true).where(role: :owner).pluck(:name),
      staff: Widget.staff.order(:name).pluck(:name),
      gadgets: Gadget.order(:label).where(role: :owner).pluck(:label),
      plain: Widget.where(role: :admin).pluck(:name)
    }
  end
end
"#,
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: %i[index]\nend\n")
        .run_ruby(
            r##"
Widget.create!(name: "alpha", role: :admin)
Widget.create!(name: "beta", role: :member)
Widget.create!(name: "gamma", role: :owner)
Gadget.create!(label: "g1", role: :guest)
Gadget.create!(label: "g2", role: :owner)
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
text = +""
body.each { |part| text << part }
want = %({"listed":["alpha","gamma"],"others":["beta","gamma"],"chained":["gamma"],"staff":["alpha","beta"],"gadgets":["g2"],"plain":["alpha"]})
raise "#{status} #{text}" unless status == 200 && text == want
puts "ALL OK"
"##,
        )
        .assert_passes();
}
