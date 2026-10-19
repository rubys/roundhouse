//! The Spinel side of `tests/filter_block_params.rs`
//! (rubys/roundhouse#998): a block filter and a lambda `if:` guard that
//! read `params` must compile and gate the action the same way on
//! Spinel. Needs a Spinel compiler, so `#[ignore]`d like its sibling
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
            "ActiveRecord::Schema.define do\n  create_table \"gadgets\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/guard\", to: \"guard#show\"\n  get \"/if_guard\", to: \"if_guard#show\"\nend\n",
        )
        .write(
            "app/controllers/guard_controller.rb",
            "class GuardController < ApplicationController\n  before_action { head :forbidden if params[:deny] == \"1\" }\n\n  def show\n    head :ok\n  end\nend\n",
        )
        .write(
            "app/controllers/if_guard_controller.rb",
            "class IfGuardController < ApplicationController\n  before_action :deny, if: -> { params[:deny] == \"1\" }\n\n  def show\n    head :ok\n  end\n\n  private\n\n  def deny\n    head :forbidden\n  end\nend\n",
        )
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn block_and_lambda_guards_read_params_on_spinel() {
    let run = app().run_spinel(
        r##"
denied = GuardController.new
denied.params = { "deny" => "1" }
denied.process_action(:show)
raise "block guard: expected 403, got #{denied.status}" unless denied.status == 403

allowed = GuardController.new
allowed.params = { "deny" => "0" }
allowed.process_action(:show)
raise "block guard: expected 200, got #{allowed.status}" unless allowed.status == 200

lambda_denied = IfGuardController.new
lambda_denied.params = { "deny" => "1" }
lambda_denied.process_action(:show)
raise "lambda guard: expected 403, got #{lambda_denied.status}" unless lambda_denied.status == 403

lambda_allowed = IfGuardController.new
lambda_allowed.params = { "deny" => "0" }
lambda_allowed.process_action(:show)
raise "lambda guard: expected 200, got #{lambda_allowed.status}" unless lambda_allowed.status == 200
puts "block and lambda guards read params (spinel)"
"##,
    );
    run.assert_passes();
    assert!(run.stdout.contains("block and lambda guards read params (spinel)"), "{}", run.stdout);
}
