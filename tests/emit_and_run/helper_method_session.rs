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

/// The request state can reach a helper without a request word in its
/// body. `@visitor ||= Current.visitor` memoizes on whatever object
/// runs it, and a class-side clone is one object for the whole
/// process: the first request's visitor answered every later one.
/// `visitor_label` reads no ivar itself but calls `visitor`, which a
/// class-side clone could not find. `greeting` is pure through the
/// private `salutation` it calls, so it is still served class-side.
#[test]
fn a_memoizing_helper_method_answers_each_request_on_its_controller() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n\n  get \"visitors/:who\", to: \"visitor_probe#show\"\n",
        )
        .write(
            "app/models/current.rb",
            "class Current < ActiveSupport::CurrentAttributes\n  attribute :visitor\nend\n",
        )
        .write(
            "app/controllers/visitor_probe_controller.rb",
            r##"class VisitorProbeController < ApplicationController
  before_action :remember_visitor
  helper_method :visitor, :visitor_label, :greeting

  def show
    @from_action = visitor
  end

  def visitor
    @visitor ||= Current.visitor
  end

  def visitor_label
    "visitor #{visitor}"
  end

  def greeting(name)
    "#{salutation}, #{name}"
  end

  private

  def remember_visitor
    Current.visitor = params[:who]
  end

  def salutation
    "hello"
  end
end
"##,
        )
        .write(
            "app/views/visitor_probe/show.html.erb",
            "<p id=\"visitor\"><%= visitor %>|<%= visitor_label %>|<%= greeting(\"ada\") %>|<%= @from_action %></p>\n<%= render \"visitor_probe/badge\" %>\n",
        )
        .write("app/views/visitor_probe/_badge.html.erb", "<span class=\"badge\"><%= visitor %></span>\n")
        .write(
            "test/controllers/visitor_probe_controller_test.rb",
            r##"require "test_helper"

class VisitorProbeControllerTest < ActionDispatch::IntegrationTest
  test "each request's helper reads its own visitor" do
    get "/visitors/ada"
    assert_response :success
    assert_includes response.body, "ada|visitor ada|hello, ada|ada"
    assert_includes response.body, "<span class=\"badge\">ada</span>"
    get "/visitors/bob"
    assert_response :success
    assert_includes response.body, "bob|visitor bob|hello, ada|bob"
    assert_includes response.body, "<span class=\"badge\">bob</span>"
  end
end
"##,
        )
        .run_test("test/controllers/visitor_probe_controller_test.rb")
        .assert_passes();
}
