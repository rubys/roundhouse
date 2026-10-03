//! Inline primitive JSON must use an encoder present in the compiled tree.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// Build generic controllers covering literal and conditional primitive payloads.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/payload\", to: \"payloads#show\"\n  get \"/list\", to: \"payloads#index\"\n  get \"/choose\", to: \"payloads#choose\"\nend\n")
        .write("app/controllers/payloads_controller.rb", r#"class PayloadsController < ApplicationController
  def show
    render json: { message: "hello\n\"world\"", count: 2, active: true, missing: nil, nested: { tags: ["one", "two"], empty: [], object: {} } }, status: 202
  end
  def index
    render json: [{ name: "first", count: 1 }, { name: "second", count: 2 }]
  end
  def choose
    render json: (params[:shape] == "array" ? ["one"] : { name: "one" })
  end
end
"#)
}

const ASSERTIONS: &str = r#"
require_relative "app/controllers/payloads_controller"
controller = PayloadsController.new
controller.process_action(:show)
raise "wrong status" unless controller.status == 202
raise "wrong content type" unless controller.content_type == "application/json"
raise controller.body unless controller.body == '{"message":"hello\n\"world\"","count":2,"active":true,"missing":null,"nested":{"tags":["one","two"],"empty":[],"object":{}}}'
controller = PayloadsController.new
controller.process_action(:index)
raise controller.body unless controller.body == '[{"name":"first","count":1},{"name":"second","count":2}]'
controller = PayloadsController.new
controller.params = {"shape" => "array"}
controller.process_action(:choose)
raise controller.body unless controller.body == '["one"]'
controller = PayloadsController.new
controller.params = {"shape" => "hash"}
controller.process_action(:choose)
raise controller.body unless controller.body == '{"name":"one"}'
puts "primitive JSON passed"
"#;

/// CRuby preserves primitive payload bytes, status, and content type.
#[test]
fn inline_primitive_json_runs() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

/// Temporal values must retain Rails serialization instead of primitive encoding.
#[test]
fn a_nested_time_keeps_rails_json_serialization() {
    app()
        .write("app/controllers/payloads_controller.rb", r#"class PayloadsController < ApplicationController
  def show
    render json: { at: Time.utc(2026, 7, 1, 12, 34, 56) }
  end
  def index
    head :no_content
  end
  def choose
    head :no_content
  end
end
"#)
        .run_ruby(r#"
require_relative "app/controllers/payloads_controller"
controller = PayloadsController.new
controller.process_action(:show)
raise controller.body unless controller.body == '{"at":"2026-07-01T12:34:56.000Z"}'
"#)
        .assert_passes();
}

/// The compiled runtime handles the same primitive and conditional payloads.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn inline_primitive_json_runs_on_spinel() {
    app().run_spinel(ASSERTIONS).assert_passes();
}
