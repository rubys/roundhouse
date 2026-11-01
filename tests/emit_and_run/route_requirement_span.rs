//! A route requirement that admits `/` spans segments, as Rails reads it
//! over the rest of the path (actionpack 8.1 `recognize_path`): `/%40.*/`
//! takes `/enc/%40a/b` whole and keeps a trailing `.json`, and `/[^.]+/`
//! takes `/nodot/a/b` but leaves `.json` to the format. Before, the router
//! matched either within one segment, so those URLs were 404s with no
//! diagnostic.

use super::emit_and_run;

const TEST_PATH: &str = "test/controllers/widgets_controller_test.rb";

#[test]
fn a_requirement_that_admits_a_slash_spans_the_rest_of_the_path() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            concat!(
                "  root \"articles#index\"\n",
                "  get \"/enc/:value\", to: \"widgets#show\", constraints: { value: /%40.*/ }\n",
                "  get \"/nodot/:value\", to: \"widgets#show\", constraints: { value: /[^.]+/ }\n",
            ),
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    render plain: \"#{params[:value]}|#{params[:format]}\"\n  end\nend\n",
        )
        .write(
            TEST_PATH,
            r#"require "test_helper"

class WidgetsControllerTest < ActionDispatch::IntegrationTest
  test "the value runs to the end of the path, the extension as Rails reads it" do
    get "/enc/%40a/b"
    assert_equal "@a/b|", response.body
    get "/enc/%40a/b.json"
    assert_equal "@a/b.json|", response.body
    get "/nodot/a/b"
    assert_equal "a/b|", response.body
    get "/nodot/a/b.json"
    assert_equal "a/b|json", response.body
  end

  test "a value the requirement does not match is no route" do
    missed = begin
      get "/enc/x/y"
      "routed"
    rescue => e
      e.message
    end
    assert_match(/No route matches/, missed)
    dotted = begin
      get "/nodot/a.b/c"
      "routed"
    rescue => e
      e.message
    end
    assert_match(/No route matches/, dotted)
  end
end
"#,
        )
        .run_test(TEST_PATH)
        .assert_passes();
}
