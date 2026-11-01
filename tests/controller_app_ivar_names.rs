//! An app's controller ivar may share a name with the response state
//! the runtime keeps. Rails keeps its own state in `@_`-prefixed ivars,
//! so with a `before_action` setting `@status`, `@body`,
//! `@content_type`, `@performed`, `@location`, `@head_response`,
//! `@headers` and `@request`, every action still answers as usual and
//! every app value survives it (actionpack 8.1.4, oracle
//! `rails-oracle.rb`: `render plain:` is 200 "ok", `redirect_to` 302,
//! `head :no_content` 204, a response header is set).
//!
//! The runtime kept its state in those names, so the two overwrote each
//! other: a render set the app's `@status` to 200 and its `@body` to
//! the text, a redirect its `@location`; the app's `@headers` made
//! `response.headers[…] =` raise, its `@performed` made `head` raise
//! DoubleRenderError, and its `@request` made `redirect_to` raise.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CONTROLLER: &str = r#"class ProbesController < ApplicationController
  before_action :set_names

  def show
    render plain: "ok"
  end

  def gone
    redirect_to "/elsewhere"
  end

  def empty
    head :no_content
  end

  def tagged
    response.headers["X-Tag"] = "1"
    render plain: "ok"
  end

  def kept
    [@status, @body, @content_type, @performed, @location, @head_response, @headers, @request]
  end

  private

  def set_names
    @status = "app status"
    @body = "app body"
    @content_type = "app content_type"
    @performed = "app performed"
    @location = "app location"
    @head_response = "app head_response"
    @headers = "app headers"
    @request = "app request"
  end
end
"#;

const ASSERTIONS: &str = r#"
require_relative "app/controllers/probes_controller"
want = ["app status", "app body", "app content_type", "app performed", "app location", "app head_response", "app headers", "app request"]
def answer(action)
  c = ProbesController.new
  c.params = {}
  c.process_action(action)
  c
end
c = answer(:show)
raise "show: #{c.status} #{c.body.inspect}" unless c.status == 200 && c.body == "ok"
raise "show kept #{c.kept.inspect}" unless c.kept == want
c = answer(:gone)
raise "gone: #{c.status} #{c.location.inspect}" unless c.status == 302 && c.location == "/elsewhere"
raise "gone kept #{c.kept.inspect}" unless c.kept == want
c = answer(:empty)
raise "empty: #{c.status}" unless c.status == 204
raise "empty kept #{c.kept.inspect}" unless c.kept == want
c = answer(:tagged)
raise "tagged: #{c.status} #{c.response.headers["X-Tag"].inspect}" unless c.status == 200 && c.response.headers["X-Tag"] == "1"
raise "tagged kept #{c.kept.inspect}" unless c.kept == want
puts "app ivars kept"
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/show\", to: \"probes#show\"\n  get \"/gone\", to: \"probes#gone\"\n  get \"/empty\", to: \"probes#empty\"\n  get \"/tagged\", to: \"probes#tagged\"\nend\n",
        )
        .write("app/controllers/probes_controller.rb", CONTROLLER)
}

#[test]
fn app_ivars_named_like_response_state_are_kept_on_ruby() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("app ivars kept"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn app_ivars_named_like_response_state_are_kept_on_spinel_with_rbs() {
    let run = app().run_spinel_with_rbs(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("app ivars kept"), "{}", run.stdout);
}
