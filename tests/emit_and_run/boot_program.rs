//! An initializer's own top-level code runs at boot (E2 of #841, host
//! initializers): a constant it assigns, statements that fill it, and a
//! top-level `def` that every class can call. Before, all of it was an
//! unconsumed-initializer warning, and a controller calling the `def` was
//! an error.

use super::emit_and_run;

const TEST_PATH: &str = "test/controllers/greetings_controller_test.rb";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "config/initializers/greetings.rb",
            concat!(
                "GREETING_PREFIX = \"Hello\"\n",
                "BOOT_GREETINGS = []\n",
                "\n",
                "def greeting_for(name)\n",
                "  \"#{GREETING_PREFIX}, #{name}\"\n",
                "end\n",
                "\n",
                "%w[boot load].each do |stage|\n",
                "  BOOT_GREETINGS << greeting_for(stage)\n",
                "end\n",
            ),
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/greetings/:id\" => \"greetings#show\"\n",
        )
        .write(
            "app/controllers/greetings_controller.rb",
            concat!(
                "class GreetingsController < ApplicationController\n",
                "  def show\n",
                "    render plain: \"#{greeting_for(params[:id])}|#{BOOT_GREETINGS.join(\",\")}\"\n",
                "  end\n",
                "end\n",
            ),
        )
}

#[test]
fn initializer_code_checks_clean() {
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn initializer_code_runs_at_boot_and_its_def_answers_everywhere() {
    app()
        .write(
            TEST_PATH,
            r#"require "test_helper"

class GreetingsControllerTest < ActionDispatch::IntegrationTest
  test "the boot statements ran and the def answers from a controller" do
    get "/greetings/alice"
    assert_equal "Hello, alice|Hello, boot,Hello, load", response.body
  end
end
"#,
        )
        .run_test(TEST_PATH)
        .assert_passes();
}
