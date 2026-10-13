//! A datetime column rendered as JSON is presented in the app's
//! `config.time_zone`, as Rails' `TimeWithZone#as_json` does.
//!
//! With `config.time_zone = "Europe/Paris"`, Rails renders a stored
//! `2026-10-12 15:38:37` (UTC) as `"2026-10-12T17:38:37.000+02:00"`, and
//! a January one with `+01:00`: the offset is the zone's at that instant,
//! so it cannot be fixed at compile time. Both the jbuilder lowerer and
//! the default `as_json` writer called `JsonBuilder.encode_datetime`,
//! which always spells the instant in UTC with a `Z`. That is Rails'
//! answer only when the zone is UTC, which is also Rails' default, so an
//! app that sets no zone (or "UTC") keeps `encode_datetime`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  resources :widgets, only: [:index, :show]
end
"#;

fn application_rb(zone_line: &str) -> String {
    format!(
        "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Shop\n  class Application < Rails::Application\n    \
         config.load_defaults 8.1\n    {zone_line}\n  end\nend\n"
    )
}

fn app(zone_line: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("config/application.rb", &application_rb(zone_line))
        .write("db/schema.rb", SCHEMA)
        .write("config/routes.rb", ROUTES)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    @widgets = Widget.all\n  end\n\n  \
             def show\n    render json: Widget.find(params[:id])\n  end\nend\n",
        )
        .write(
            "app/views/widgets/index.json.jbuilder",
            "json.widgets(@widgets) do |widget|\n  json.partial!(widget, partial: \"widgets/widget\", as: :widget)\nend\n",
        )
        .write(
            "app/views/widgets/_widget.json.jbuilder",
            "json.call(widget, :id, :name, :created_at)\njson.touched_at(widget.updated_at)\n",
        )
}

fn partial(zone_line: &str, target: BuildTarget) -> String {
    let (emitted, _errors) = app(zone_line).emit(target);
    std::fs::read_to_string(emitted.join("app/views/widgets/_widget_json.rb")).expect("partial emitted")
}

#[test]
fn a_zoned_app_renders_temporal_columns_through_json_time() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let widget = partial("config.time_zone = \"Europe/Paris\"", target);
        for read in ["json_time(widget.created_at_raw)", "json_time(widget.updated_at_raw)"] {
            assert!(widget.contains(read), "{target:?}: expected `{read}`:\n{widget}");
        }
        assert!(!widget.contains("encode_datetime"), "{target:?}: UTC encoder left:\n{widget}");
    }
}

#[test]
fn a_utc_app_keeps_the_utc_encoder() {
    for zone_line in ["", "config.time_zone = \"UTC\"", "config.time_zone = 'Etc/UTC'"] {
        let widget = partial(zone_line, BuildTarget::Ruby);
        assert!(
            widget.contains("encode_datetime(widget.created_at_raw)"),
            "`{zone_line}`: expected the UTC encoder:\n{widget}"
        );
        assert!(!widget.contains("json_time"), "`{zone_line}`:\n{widget}");
    }
}

#[test]
fn a_zoned_app_renders_rails_offsets() {
    app("config.time_zone = \"Europe/Paris\"")
        .run_ruby(
            r##"
Widget.create!(name: "summer", created_at: Time.utc(2026, 10, 12, 15, 38, 37), updated_at: Time.utc(2026, 1, 12, 15, 38, 37))
def get(path)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  raise "#{path}: status #{status} #{text}" unless status == 200
  text
end
index = get("/widgets.json")
want = %("created_at":"2026-10-12T17:38:37.000+02:00","touched_at":"2026-01-12T16:38:37.000+01:00")
raise "index: #{index}" unless index.include?(want)
show = get("/widgets/1.json")
want = %("created_at":"2026-10-12T17:38:37.000+02:00","updated_at":"2026-01-12T16:38:37.000+01:00")
raise "show: #{show}" unless show.include?(want)
puts "ALL OK"
"##,
        )
        .assert_passes();
}
