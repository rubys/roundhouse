//! campfire's `default_url_options` passes `port: request.optional_port`
//! and drops blanks: at the scheme's standard port that is nil, so the
//! options carry no port at all.

use super::emit_and_run;

#[test]
fn optional_port_is_nil_at_the_standard_port_in_default_url_options() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/where\", to: \"wheres#show\"\n",
        )
        .write(
            "app/controllers/wheres_controller.rb",
            r##"class WheresController < ApplicationController
  def show
    render plain: "#{request.optional_port.inspect} #{default_url_options.inspect}"
  end

  def default_url_options
    { port: request.optional_port }.compact_blank
  end
end
"##,
        )
        .write(
            "test/controllers/wheres_controller_test.rb",
            r#"require "test_helper"

class WheresControllerTest < ActionDispatch::IntegrationTest
  test "no port at the standard one" do
    get "/where"
    assert_equal "nil {}", response.body
  end
end
"#,
        )
        .run_test("test/controllers/wheres_controller_test.rb")
        .assert_passes();
}
