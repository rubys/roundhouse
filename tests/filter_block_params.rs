//! A block or lambda `before_action`, and a lambda `if:` / `unless:`
//! guard, read `params` exactly as the action does (rubys/roundhouse#998).
//!
//! Rails runs these with `instance_exec` on the controller, so `params`
//! in them is the request's indifferent-access Parameters. Their bodies
//! are spliced into the synthesized `process_action`, which never went
//! through `rewrite_params`: `params[:id]` stayed a Symbol-keyed read on
//! the String-keyed params Hash and answered nil. `Widget.find(nil)`
//! answered 404, and a guard written `head :forbidden if params[:deny]
//! == "1"` never fired, so the access check let every request through.
//!
//! The expected answers are Rails 8.1.4's on the same app, booted
//! in-process with `Rack::MockRequest`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const SHOW: &str = "  def show\n    render json: { name: @widget.name }\n  end\n";

fn app() -> emit_and_run::Overlay {
    let widget_filter = |name: &str, filter: &str| {
        format!("class {name}Controller < ApplicationController\n{filter}\n\n{SHOW}end\n")
    };
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        )
        .write(
            "app/controllers/brace_controller.rb",
            &widget_filter("Brace", "  before_action { @widget = Widget.find(params[:id]) }"),
        )
        .write(
            "app/controllers/do_end_controller.rb",
            &widget_filter(
                "DoEnd",
                "  before_action do\n    @widget = Widget.find(params[:id])\n  end",
            ),
        )
        .write(
            "app/controllers/stabby_controller.rb",
            &widget_filter("Stabby", "  before_action -> { @widget = Widget.find(params[:id]) }"),
        )
        .write(
            "app/controllers/scoped_controller.rb",
            &widget_filter(
                "Scoped",
                "  before_action(only: :show) { @widget = Widget.find(params[:id]) }",
            ),
        )
        // The control: the same filter with a Symbol target, inlined into
        // the action, already read `params` correctly.
        .write(
            "app/controllers/named_controller.rb",
            "class NamedController < ApplicationController\n  before_action :set_widget\n\n  def show\n    render json: { name: @widget.name }\n  end\n\n  private\n\n  def set_widget\n    @widget = Widget.find(params[:id])\n  end\nend\n",
        )
        .write(
            "app/controllers/guard_controller.rb",
            "class GuardController < ApplicationController\n  before_action { head :forbidden if params[:deny] == \"1\" }\n\n  def show\n    render plain: \"shown\"\n  end\nend\n",
        )
        .write(
            "app/controllers/if_guard_controller.rb",
            "class IfGuardController < ApplicationController\n  before_action :deny, if: -> { params[:deny] == \"1\" }\n\n  def show\n    render plain: \"shown\"\n  end\n\n  private\n\n  def deny\n    head :forbidden\n  end\nend\n",
        )
        .write(
            "app/controllers/unless_guard_controller.rb",
            "class UnlessGuardController < ApplicationController\n  before_action :deny, unless: -> { params[:allow] == \"1\" }\n\n  def show\n    render plain: \"shown\"\n  end\n\n  private\n\n  def deny\n    head :forbidden\n  end\nend\n",
        )
        .write(
            "app/controllers/block_if_controller.rb",
            "class BlockIfController < ApplicationController\n  before_action(if: -> { params[:deny] == \"1\" }) { head :forbidden }\n\n  def show\n    render plain: \"shown\"\n  end\nend\n",
        )
        // A `rescue_from` block is spliced into the same dispatcher and
        // read `params` the same wrong way.
        .write(
            "app/controllers/rescued_controller.rb",
            "class RescuedController < ApplicationController\n  rescue_from(ActiveRecord::RecordNotFound) { head(params[:id] == \"9\" ? :not_found : :gone) }\n\n  def show\n    @widget = Widget.find(params[:id])\n    render plain: @widget.name\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  get "brace/:id", to: "brace#show"
  get "do_end/:id", to: "do_end#show"
  get "stabby/:id", to: "stabby#show"
  get "scoped/:id", to: "scoped#show"
  get "named/:id", to: "named#show"
  get "guard", to: "guard#show"
  get "if_guard", to: "if_guard#show"
  get "unless_guard", to: "unless_guard#show"
  get "block_if", to: "block_if#show"
  get "rescued/:id", to: "rescued#show"
end
"#,
        )
}

/// Every row answers what Rails 8.1.4 answers.
#[test]
fn block_and_lambda_filters_read_params_as_the_action_does() {
    app()
        .run_ruby(
            r##"
require "stringio"
Widget.create!(name: "one")
expected = {
  "/brace/1" => [200, %({"name":"one"})],
  "/do_end/1" => [200, %({"name":"one"})],
  "/stabby/1" => [200, %({"name":"one"})],
  "/scoped/1" => [200, %({"name":"one"})],
  "/named/1" => [200, %({"name":"one"})],
  "/guard" => [200, "shown"],
  "/guard?deny=1" => [403, ""],
  "/if_guard" => [200, "shown"],
  "/if_guard?deny=1" => [403, ""],
  "/unless_guard" => [403, ""],
  "/unless_guard?allow=1" => [200, "shown"],
  "/block_if" => [200, "shown"],
  "/block_if?deny=1" => [403, ""],
  "/rescued/1" => [200, "one"],
  "/rescued/9" => [404, ""],
}
failures = expected.filter_map do |url, (status, body)|
  path, query = url.split("?", 2)
  got_status, _headers, got_body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query.to_s, "rack.input" => StringIO.new(""))
  text = +""
  got_body.each { |part| text << part }
  "#{url}: expected #{status} #{body.inspect}, got #{got_status} #{text.inspect}" unless got_status == status && text == body
end
raise failures.join("\n") unless failures.empty?
puts "ALL OK"
"##,
        )
        .assert_passes();
}

/// The strict targets consume the same lowered dispatcher: the rewritten
/// `@params` reads must emit there without new errors.
#[test]
fn the_rewritten_dispatcher_emits_for_rust_and_typescript() {
    for target in [BuildTarget::Rust, BuildTarget::Typescript] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}
