//! A `return` inside a `before_action` ends the filter, not the action.
//!
//! `inline_before_filters` splices an own-private filter's body into each
//! action it applies to. A filter that leaves early with `return`
//! (`return if params["short"] == "1"` half-way through) then returned
//! from the ACTION: the request answered 200 with an empty body, the
//! action never ran. In Rails the filter's `return` only ends that
//! method; the chain halts only when the filter rendered or redirected
//! (`performed?`). The Spinel side is
//! `tests/before_action_return_spinel.rs`.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"secrets\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/secret\", to: \"secrets#show\"\n  get \"/login\", to: \"secrets#login\"\nend\n",
        )
        .write(
            "app/controllers/secrets_controller.rb",
            r#"class SecretsController < ApplicationController
  before_action :require_login, only: :show
  before_action :load_label, only: :show

  def show
    render plain: "secret #{@label}#{@suffix}"
  end

  def login
    render plain: "login"
  end

  private

  def logged_in?
    params["token"].to_s == "abc"
  end

  def require_login
    return if logged_in?
    redirect_to "/login"
  end

  def load_label
    @label = "for you"
    @suffix = ""
    return if params["short"].to_s == "1"
    @suffix = ", in full"
  end
end
"#,
        )
}

const ASSERTIONS: &str = r#"
def get(query)
  status, headers, body = Main.run_rack(
    "REQUEST_METHOD" => "GET", "PATH_INFO" => "/secret", "QUERY_STRING" => query,
    "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
  )
  [status, headers, body.join]
end

status, _, body = get("token=abc")
raise "signed in: status #{status}, want 200" unless status == 200
raise "signed in: body #{body.inspect}" unless body == "secret for you, in full"

status, _, body = get("token=abc&short=1")
raise "short: status #{status}, want 200" unless status == 200
raise "short: body #{body.inspect}" unless body == "secret for you"

status, headers, body = get("")
raise "signed out: status #{status}, want 302" unless status == 302
raise "signed out: location #{headers["location"].inspect}" unless headers["location"].to_s.end_with?("/login")
raise "signed out: action ran, body #{body.inspect}" if body.include?("secret")
puts "a before_action's return ended only the filter"
"#;

#[test]
fn a_before_actions_return_ends_the_filter_not_the_action() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("a before_action's return ended only the filter"), "{}", run.stdout);
}
