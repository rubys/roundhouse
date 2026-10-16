//! Inline `render json:` values outside the primitive path reach
//! `ActionController::JsonRender.encode` on the spinel tree too. The
//! native run needs a Spinel compiler, so it is `#[ignore]`d like its
//! sibling suites; CI's `spinel-framework` job runs it with `--ignored`
//! (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/settings\", to: \"payloads#settings\"\n  get \"/count\", to: \"payloads#count\"\n  get \"/stamp\", to: \"payloads#stamp\"\nend\n")
        .write("app/models/badge.rb", r#"class Badge
  def initialize(label)
    @label = label
  end

  def as_json(options = nil)
    { "label" => @label, "icon" => "<i>" }
  end
end
"#)
        .write("app/controllers/payloads_controller.rb", r#"class PayloadsController < ApplicationController
  def settings
    render json: { theme: "dark", saved: JSON.parse('{"size":12,"tags":["a","<b>"],"extra":null}') }
  end

  def count
    render json: (params[:empty] ? nil : 3)
  end

  def stamp
    render json: { at: Time.utc(2026, 7, 1, 12, 34, 56), badge: Badge.new("new"), state: :open }
  end
end
"#)
}

/// Each action's value is untyped, a scalar, or holds objects, so none
/// takes the primitive `JSON.generate` path.
#[test]
fn the_spinel_tree_loads_the_encoder_its_controllers_call() {
    let (emitted, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let controller = std::fs::read_to_string(emitted.join("app/controllers/payloads_controller.rb"))
        .expect("emitted payloads controller");
    assert_eq!(controller.matches("ActionController::JsonRender.encode(").count(), 3, "{controller}");
    let boot = std::fs::read_to_string(emitted.join("boot.rb")).expect("emitted boot.rb");
    assert!(boot.contains("require_relative \"runtime/action_controller_json_render\""), "{boot}");
    let encoder = std::fs::read_to_string(emitted.join("runtime/action_controller_json_render.rb"))
        .expect("emitted encoder");
    assert!(encoder.contains("def self.encode(value)"), "{encoder}");
}

/// The bytes are the CRuby tree's for the same actions, and Rails 8.1.4's
/// under `load_defaults 8.1`: `as_json` first, a Time as `iso8601(3)`, a
/// Symbol as its name, `<`, `>` and `&` not escaped.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn values_outside_the_primitive_path_render_as_rails_does() {
    app()
        .run_spinel(r#"
require_relative "app/controllers/payloads_controller"
controller = PayloadsController.new
controller.process_action(:settings)
raise "wrong content type" unless controller.content_type == "application/json"
raise controller.body unless controller.body == '{"theme":"dark","saved":{"size":12,"tags":["a","<b>"],"extra":null}}'
controller = PayloadsController.new
controller.process_action(:count)
raise controller.body unless controller.body == "3"
controller = PayloadsController.new
controller.params = {"empty" => "1"}
controller.process_action(:count)
raise controller.body unless controller.body == "null"
controller = PayloadsController.new
controller.process_action(:stamp)
raise controller.body unless controller.body == '{"at":"2026-07-01T12:34:56.000Z","badge":{"label":"new","icon":"<i>"},"state":"open"}'
puts "json render passed"
"#)
        .assert_passes();
}
