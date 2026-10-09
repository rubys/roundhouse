//! `self.response_body =` and `response.content_type =` in an action, the
//! way campfire's MessagesController serves a page it rendered to a
//! string. Rails answers the assigned body with that content type, and a
//! body assigned in a `before_action` halts the chain.

use super::emit_and_run;

#[test]
fn an_assigned_response_body_is_the_response() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/raw\", to: \"raws#show\"\n  get \"/early\", to: \"raws#early\"\n  get \"/away\", to: \"raws#away\"\n  get \"/cleared\", to: \"raws#cleared\"\n",
        )
        .write(
            "app/controllers/raws_controller.rb",
            r#"class RawsController < ApplicationController
  before_action :answer_early, only: :early
  before_action :send_away, only: :away

  def show
    body = "<p>prebuilt</p>"
    response.content_type = "text/html"
    self.response_body = body
  end

  def early
    render plain: "the action ran"
  end

  def away
    render plain: "the action ran"
  end

  def cleared
    self.response_body = nil
  end

  private
    def answer_early
      self.response_body = "from the filter"
    end

    def send_away
      self.redirect_to "/raw"
    end
end
"#,
        )
        .write("app/views/raws/cleared.html.erb", "<p>from the template</p>\n")
        .write(
            "test/controllers/raws_controller_test.rb",
            r#"require "test_helper"

class RawsControllerTest < ActionDispatch::IntegrationTest
  test "an assigned body is the response" do
    get "/raw"
    assert_response :success
    assert_equal "<p>prebuilt</p>", response.body
    assert_equal "text/html", response.media_type
  end

  test "a body assigned in a before_action halts the chain" do
    get "/early"
    assert_equal "from the filter", response.body
  end

  test "a filter's self.redirect_to halts the chain" do
    get "/away"
    assert_redirected_to "/raw"
  end

  test "a nil body is no response, so the template still renders" do
    get "/cleared"
    assert_response :success
    assert_includes response.body, "from the template"
  end
end
"#,
        )
        .run_test("test/controllers/raws_controller_test.rb")
        .assert_passes();
}
