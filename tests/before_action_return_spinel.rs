//! The Spinel side of `tests/emit_and_run/before_action_return.rs`: a
//! `return` inside a `before_action` ends the filter, not the action.
//! Needs a Spinel compiler, so it is `#[ignore]`d like its sibling
//! suites; CI's `spinel-framework` job runs it with `--ignored`
//! (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

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
signed_in = SecretsController.new
signed_in.params = { "token" => "abc" }
signed_in.process_action(:show)
raise "signed in: status #{signed_in.status}, want 200" unless signed_in.status == 200
raise "signed in: body #{signed_in.body.inspect}" unless signed_in.body == "secret for you, in full"

short = SecretsController.new
short.params = { "token" => "abc", "short" => "1" }
short.process_action(:show)
raise "short: status #{short.status}, want 200" unless short.status == 200
raise "short: body #{short.body.inspect}" unless short.body == "secret for you"

signed_out = SecretsController.new
signed_out.params = {}
signed_out.process_action(:show)
raise "signed out: status #{signed_out.status}, want 302" unless signed_out.status == 302
raise "signed out: action ran, body #{signed_out.body.inspect}" if signed_out.body.include?("secret")
puts "a before_action's return ended only the filter"
"#;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_before_actions_return_ends_the_filter_not_the_action_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("a before_action's return ended only the filter"), "{}", run.stdout);
}
