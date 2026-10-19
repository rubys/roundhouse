use super::emit_and_run;

#[test]
fn an_after_action_can_check_whether_the_response_redirects() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/redirect_probe\", to: \"redirect_probes#show\"\n  post \"/redirect_probe\", to: \"redirect_probes#create\"\n  get \"/redirect_probe/cached\", to: \"redirect_probes#cached\"\n",
        )
        .write(
            "app/controllers/redirect_probes_controller.rb",
            r#"class RedirectProbesController < ApplicationController
  after_action :record_redirect

  def show
    head :ok
  end

  def create
    redirect_to "/articles", status: :see_other
  end

  def cached
    head :not_modified
  end

  private

  def record_redirect
    response.headers["X-Redirect"] = response.redirect? ? "yes" : "no"
  end
end
"#,
        )
        .write(
            "test/controllers/redirect_probes_controller_test.rb",
            r#"require "test_helper"

class RedirectProbesControllerTest < ActionDispatch::IntegrationTest
  test "the after_action sees an ordinary response" do
    get "/redirect_probe"
    assert_response :success
    assert_equal "no", response.headers["x-redirect"]
  end

  test "the after_action sees a redirect" do
    post "/redirect_probe"
    assert_response :see_other
    assert_equal "yes", response.headers["x-redirect"]
  end

  test "a not-modified response is not a redirect" do
    get "/redirect_probe/cached"
    assert_response :not_modified
    assert_equal "no", response.headers["x-redirect"]
  end
end
"#,
        )
        .run_test("test/controllers/redirect_probes_controller_test.rb")
        .assert_passes();
}
