//! Regression: `protect_from_forgery unless: -> { request.format.json? }`
//! — Rails' own API-controller guide idiom.
//!
//! Filter `if:`/`unless:` lambdas, block-form filter bodies, and
//! `rescue_from` handlers are spliced into the synthesized
//! `process_action` rather than flowing through `lower_action_body`,
//! so without `rewrite_request_format` over that finished dispatcher
//! body, a literal `request.format.json?` survived to run time:
//! `request` is nil outside a real dispatch (unit-style
//! `controller.process_action` calls), and `Kernel#format` is private,
//! so every dispatched action raised `NoMethodError` — or, behind a
//! real Request, `undefined method 'json?' for an instance of String`
//! (shared `ActionDispatch::Request#format` is a plain String reader).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn protect_from_forgery_unless_request_format_json_does_not_raise() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  protect_from_forgery unless: -> { request.format.json? }\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root \"widgets#show\"\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    render json: { ok: true }\n  end\nend\n",
        )
        .run_ruby(
            r#"
require_relative "app/controllers/widgets_controller"

# A JSON request: the `unless:` lambda must answer true and skip the
# forgery check entirely, so the action runs. This is the exact call
# the issue reports raising NoMethodError before any action body ran.
controller = WidgetsController.new
controller.params = {}
controller.request_format = :json
controller.process_action(:show)
raise "json request: #{controller.body.inspect} status=#{controller.status}" unless controller.body == "{\"ok\":true}" && controller.status == 200

# An html POST: the lambda must answer false, so the forgery check
# actually runs (and, with no authenticity token, blocks the action) --
# proving the rewrite did not just make the guard permanently skip.
ActionController::Base.allow_forgery_protection = true
blocked = WidgetsController.new
blocked.params = {}
blocked.request_format = :html
blocked.request_method = "POST"
blocked.process_action(:show)
raise "html POST should be blocked by the forgery check: status=#{blocked.status} body=#{blocked.body.inspect}" unless blocked.status == 422
ActionController::Base.allow_forgery_protection = false
puts "protect_from_forgery unless request.format.json? passed"
"#,
        )
        .assert_passes();
}
