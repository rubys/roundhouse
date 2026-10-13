//! A datetime column read in a namespaced jbuilder view
//! (`app/views/api/v1/widgets/`) renders Rails' `xmlschema(3)`, as the
//! same partial does at the top level.
//!
//! `columns_for_arg` found the template's model by camelizing the whole
//! view directory, which for `api/v1/widgets` names no model, so the
//! partial had no column types: `json.call(widget, :created_at)` and
//! `json.touched_at(widget.updated_at)` went through `encode_value`,
//! whose fallback is `Time#to_s` (`"2026-10-11 18:20:32 UTC"`). Rails
//! renders `"2026-10-11T18:20:32.000Z"`. The record is named by the
//! directory's last segment, as Rails' partial lookup implies (the
//! namespace is the controller's); a namespaced model is tried first.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
  create_table "gadgets", force: :cascade do |t|
    t.string "name", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  namespace :api do
    namespace :v1 do
      resources :widgets, only: [:index]
    end
  end
  resources :gadgets, only: [:index]
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("config/routes.rb", ROUTES)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/models/gadget.rb", "class Gadget < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/gadgets_controller.rb",
            "class GadgetsController < ApplicationController\n  def index\n    @gadgets = Gadget.all\n  end\nend\n",
        )
        .write(
            "app/controllers/api/v1/widgets_controller.rb",
            "class Api::V1::WidgetsController < ApplicationController\n  def index\n    @widgets = Widget.all\n  end\nend\n",
        )
        .write(
            "app/views/gadgets/index.json.jbuilder",
            "json.gadgets(@gadgets) do |gadget|\n  json.partial!(gadget, partial: \"gadgets/gadget\", as: :gadget)\nend\n",
        )
        .write(
            "app/views/gadgets/_gadget.json.jbuilder",
            "json.call(gadget, :id, :name, :created_at)\njson.touched_at(gadget.updated_at)\n",
        )
        .write(
            "app/views/api/v1/widgets/index.json.jbuilder",
            "json.widgets(@widgets) do |widget|\n  json.partial!(widget, partial: \"api/v1/widgets/widget\", as: :widget)\nend\n",
        )
        .write(
            "app/views/api/v1/widgets/_widget.json.jbuilder",
            "json.call(widget, :id, :name, :created_at)\njson.touched_at(widget.updated_at)\n",
        )
}

#[test]
fn namespaced_partial_encodes_temporal_columns_like_a_top_level_one() {
    let (emitted, _errors) = app().emit(BuildTarget::Spinel);
    let widget = std::fs::read_to_string(emitted.join("app/views/api/v1/widgets/_widget_json.rb"))
        .expect("namespaced partial emitted");
    for read in ["encode_datetime(widget.created_at_raw)", "encode_datetime(widget.updated_at_raw)"] {
        assert!(widget.contains(read), "expected `{read}` in the namespaced partial:\n{widget}");
    }
    assert!(!widget.contains("encode_value(widget.created_at)"), "Time#to_s fallback left:\n{widget}");
}

#[test]
fn namespaced_partial_renders_xmlschema_timestamps() {
    app()
        .run_ruby(
            r##"
Widget.create!(name: "alpha")
Gadget.create!(name: "beta")
def get(path)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  raise "#{path}: status #{status} #{text}" unless status == 200
  text
end
stamp = /\A"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z"\z/
[["/api/v1/widgets.json", "alpha"], ["/gadgets.json", "beta"]].each do |path, name|
  text = get(path)
  m = text.match(/"name":"#{name}","created_at":("[^"]*"),"touched_at":("[^"]*")/)
  raise "#{path}: #{text}" unless m
  raise "#{path} created_at #{m[1]}" unless m[1].match?(stamp)
  raise "#{path} touched_at #{m[2]}" unless m[2].match?(stamp)
end
puts "ALL OK"
"##,
        )
        .assert_passes();
}
