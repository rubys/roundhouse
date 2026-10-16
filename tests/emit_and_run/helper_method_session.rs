//! A `helper_method` that reads the request's state (the session) runs
//! on the controller handling the request, as Rails' helper proxy runs
//! it. A helper method pure over its arguments gets a class-side clone
//! the view can call without a controller; one that reads `session`
//! must not, since a class has no request.

use super::emit_and_run;

#[test]
fn a_session_reading_helper_method_runs_on_the_request_controller() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n\n  get \"probe/sign_in\", to: \"session_probe#sign_in\"\n  get \"probe\", to: \"session_probe#show\"\n",
        )
        .write(
            "app/controllers/session_probe_controller.rb",
            r##"class SessionProbeController < ApplicationController
  helper_method :signed_in_as

  def sign_in
    session[:who] = "ada"
    redirect_to "/probe"
  end

  def show
  end

  def signed_in_as
    session[:who] || "nobody"
  end
end
"##,
        )
        .write("app/views/session_probe/show.html.erb", "<p id=\"who\">signed in as <%= signed_in_as %></p>\n")
        .write(
            "test/controllers/session_probe_controller_test.rb",
            r##"require "test_helper"

class SessionProbeControllerTest < ActionDispatch::IntegrationTest
  test "the helper reads this request's session" do
    get "/probe"
    assert_response :success
    assert_includes response.body, "signed in as nobody"
    get "/probe/sign_in"
    get "/probe"
    assert_includes response.body, "signed in as ada"
  end
end
"##,
        )
        .run_test("test/controllers/session_probe_controller_test.rb")
        .assert_passes();
}
