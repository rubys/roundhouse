//! `where(enum_attr: CONSTANT)` maps a constant list of labels.
//!
//! Rails casts every element of a `where` value through the enum type,
//! whether the list is written inline or held in a constant
//! (`ROLES = %i[admin owner].freeze; Widget.where(role: ROLES)`). The
//! label rewrite saw only literal Symbols and literal lists, so the
//! constant's Symbols reached the SQL and the query matched no row.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn where_maps_an_enum_label_constant() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.integer \"role\", default: 0, null: false\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  STAFF = [:member, :admin].freeze\n\n  enum :role, %i[member admin owner]\n\n  def self.staff\n    where(role: STAFF)\n  end\nend\n",
        )
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  ROLES = %i[admin owner].freeze

  def admins
    render json: {
      names: Widget.where(role: ROLES).order(:name).pluck(:name),
      others: Widget.where.not(role: ROLES).pluck(:name),
      staff: Widget.staff.order(:name).pluck(:name),
      qualified: Widget.where(role: Widget::STAFF).count
    }
  end
end
"#,
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"widgets/admins\", to: \"widgets#admins\"\nend\n")
        .run_ruby(
            r##"
Widget.create!(name: "alpha", role: :admin)
Widget.create!(name: "beta", role: :member)
Widget.create!(name: "gamma", role: :owner)
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets/admins", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
text = +""
body.each { |part| text << part }
want = %({"names":["alpha","gamma"],"others":["beta"],"staff":["alpha","beta"],"qualified":2})
raise "#{status} #{text}" unless status == 200 && text == want
puts "ALL OK"
"##,
        )
        .assert_passes();
}
