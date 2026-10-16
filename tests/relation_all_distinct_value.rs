//! `Relation#all`, and `Relation#distinct(value)` taking its argument.
//!
//! Rails' `QueryMethods#distinct(value = true)` turns DISTINCT on for
//! any truthy value (`distinct(:name)` is `distinct(true)`, not a column
//! list) and off for `false`; `Relation#all` answers the relation as a
//! new one, so a chain written for a class (`Widget.all`) reads on a
//! scope too. The runtime Relation had `distinct` with no parameter
//! (`distinct(:name)` raised ArgumentError) and no `all` (NoMethodError).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn relation_all_and_distinct_with_a_value() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.integer \"price\", default: 0\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  def names
    render json: { names: Widget.distinct(:name).pluck(:name).sort, all: Widget.where(price: 0..).distinct(false).pluck(:name).size }
  end

  def extra
    render json: { count: Widget.where(price: 0..).all.count, cheap: Widget.where(price: ..5).all.order(:name).pluck(:name) }
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"widgets/names\", to: \"widgets#names\"\n  get \"widgets/extra\", to: \"widgets#extra\"\nend\n",
        )
        .run_ruby(
            r##"
Widget.create!(name: "alpha", price: 3)
Widget.create!(name: "beta", price: 9)
Widget.create!(name: "beta", price: 1)
def probe(path)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  [status, text]
end
got = probe("/widgets/names")
raise "names #{got.inspect}" unless got == [200, %({"names":["alpha","beta"],"all":3})]
got = probe("/widgets/extra")
raise "extra #{got.inspect}" unless got == [200, %({"count":3,"cheap":["alpha","beta"]})]
puts "ALL OK"
"##,
        )
        .assert_passes();
}
