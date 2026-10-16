//! `json.merge!(record.attributes.except(...))` writes the record's
//! attributes as pairs.
//!
//! Rails' `attributes` is the record's column values under String keys,
//! in column order, an enum column under its label. Merging it (usually
//! narrowed with `except` or `slice`) is a common way to render a record
//! whole. The template's values arrive untyped, so the `merge!` was
//! refused (a raise at render). It is the same object `json.extract!`
//! builds over the remaining columns, so it lowers to that, wherever the
//! keys are known: the record is the template's own, the narrowing keys
//! are literals, and no other statement of the object sets one of them.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn merge_of_a_records_attributes_writes_its_columns() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.integer "role", default: 0, null: false
    t.integer "price", default: 0
    t.boolean "active", default: true
    t.timestamps
  end
end
"#,
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  enum :role, %i[member admin owner]\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  def show
    @widget = Widget.find(params[:id])
  end

  def sliced
    @widget = Widget.find(params[:id])
  end

  def clash
    @widget = Widget.find(params[:id])
  end
end
"#,
        )
        .write(
            "app/views/widgets/show.json.jbuilder",
            "json.merge!(@widget.attributes.except(\"created_at\", \"updated_at\", \"id\", \"active\"))\njson.done true\n",
        )
        .write(
            "app/views/widgets/sliced.json.jbuilder",
            "json.kind \"widget\"\njson.merge!(@widget.attributes.except(:id).slice(\"price\", \"id\", \"name\"))\n",
        )
        .write(
            "app/views/widgets/clash.json.jbuilder",
            "json.name \"x\"\njson.merge!(@widget.attributes.slice(\"name\"))\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: %i[show] do\n    member do\n      get :sliced\n      get :clash\n    end\n  end\nend\n",
        )
        .run_ruby(
            r##"
def probe(path)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "HTTP_ACCEPT" => "application/json", "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  [status, text]
end
w = Widget.create!(name: "alpha", role: :admin, price: 3)
got = probe("/widgets/#{w.id}.json")
raise "show #{got.inspect}" unless got == [200, %({"name":"alpha","role":"admin","price":3,"done":true})]
# `except(:id)` removes nothing from a String-keyed Hash; `slice` answers
# in its arguments' order.
got = probe("/widgets/#{w.id}/sliced.json")
raise "sliced #{got.inspect}" unless got == [200, %({"kind":"widget","price":3,"id":#{w.id},"name":"alpha"})]
# A key the object already sets: Jbuilder merges it into the first
# pair, so this is still refused rather than written twice.
refused = begin
  probe("/widgets/#{w.id}/clash.json")
  nil
rescue RuntimeError => e
  e.message
end
raise "clash #{refused.inspect}" unless refused.to_s.include?("merge! not supported")
puts "ALL OK"
"##,
        )
        .assert_passes();
}

/// Rails leaves a model's `ignored_columns` out of `attributes`; the
/// table's columns are then not its attributes, so the merge is refused
/// (a raise naming it) rather than written with the ignored column.
#[test]
fn merge_of_attributes_with_ignored_columns_is_refused() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.integer \"price\", default: 0\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  self.ignored_columns = [\"price\"]\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n  end\nend\n",
        )
        .write("app/views/widgets/show.json.jbuilder", "json.merge!(@widget.attributes.except(\"id\"))\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: %i[show]\nend\n")
        .run_ruby(
            r##"
w = Widget.create!(name: "alpha")
refused = begin
  Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets/#{w.id}.json", "QUERY_STRING" => "", "HTTP_ACCEPT" => "application/json", "rack.input" => StringIO.new(""))
  nil
rescue RuntimeError => e
  e.message
end
raise "ignored_columns #{refused.inspect}" unless refused.to_s.include?("merge! not supported")
puts "ALL OK"
"##,
        )
        .assert_passes();
}
