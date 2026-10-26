#[test]
fn helper_props_preserve_deferral_optional_always_and_evaluation_count() {
    super::emit_and_run::empty_app()
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get '/pages/show', to: 'pages#show'\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            r#"class ApplicationController < ActionController::Base
  inertia_share shared: -> { shared_prop }

  private

  def shared_prop
    @shared_calls += 1
    InertiaRails.defer(group: "shared") { 13 }
  end
end
"#,
        )
        .write(
            "app/controllers/pages_controller.rb",
            r#"class PagesController < ApplicationController
  def show
    @calls = 0
    @shared_calls = 0
    local = deferred_prop(7)
    render inertia: "pages/show", props: {
      title: "page",
      stats: local,
      other: deferred_prop(11),
      secret: optional_prop,
      kept: always_prop,
      mixed: mixed_prop,
      sharedCalls: -> { @shared_calls },
      calls: -> { @calls }
    }
  end

  private

  def deferred_prop(total)
    @calls += 1
    InertiaRails.defer(group: "statistics") { { total: total } }
  end

  def optional_prop
    InertiaRails.optional { "requested" }
  end

  def always_prop
    InertiaRails.always { "kept" }
  end

  def mixed_prop
    if params[:plain] == "yes"
      "plain"
    else
      InertiaRails.defer { "deferred" }
    end
  end
end
"#,
        )
        .write(
            "test/controllers/pages_controller_test.rb",
            r#"require "test_helper"

class PagesControllerTest < ActionDispatch::IntegrationTest
  test "a first visit lists helper props in their deferred groups" do
    get "/pages/show", headers: { "X-Inertia" => "true" }
    assert_response :success
    page = response.parsed_body
    assert_equal({ "title" => "page", "kept" => "kept", "sharedCalls" => 1, "calls" => 2 }, page["props"])
    assert_equal({ "shared" => ["shared"], "statistics" => ["stats", "other"], "default" => ["mixed"] }, page["deferredProps"])
    assert_equal ["shared"], page["sharedProps"]
  end

  test "a partial reload evaluates requested helper props once" do
    get "/pages/show", headers: {
      "X-Inertia" => "true",
      "X-Inertia-Partial-Component" => "pages/show",
      "X-Inertia-Partial-Data" => "stats,secret,calls,shared,sharedCalls"
    }
    assert_response :success
    assert_equal({ "shared" => 13, "stats" => { "total" => 7 }, "secret" => "requested", "kept" => "kept", "sharedCalls" => 1, "calls" => 2 }, response.parsed_body["props"])
    assert_nil response.parsed_body["deferredProps"]
  end

  test "except cannot remove an always helper prop" do
    get "/pages/show", headers: {
      "X-Inertia" => "true",
      "X-Inertia-Partial-Component" => "pages/show",
      "X-Inertia-Partial-Except" => "title,stats,other,secret,mixed,calls,shared,sharedCalls,kept"
    }
    assert_response :success
    assert_equal({ "kept" => "kept" }, response.parsed_body["props"])
  end

  test "an unrequested shared lambda does not run its helper" do
    get "/pages/show", headers: {
      "X-Inertia" => "true",
      "X-Inertia-Partial-Component" => "pages/show",
      "X-Inertia-Partial-Data" => "title,sharedCalls,calls"
    }
    assert_response :success
    assert_equal({ "title" => "page", "kept" => "kept", "sharedCalls" => 0, "calls" => 2 }, response.parsed_body["props"])
  end

  test "a helper can return either an ordinary value or a deferred prop" do
    get "/pages/show?plain=yes", headers: { "X-Inertia" => "true" }
    assert_response :success
    assert_equal "plain", response.parsed_body["props"]["mixed"]
    assert_equal({ "shared" => ["shared"], "statistics" => ["stats", "other"] }, response.parsed_body["deferredProps"])
  end
end
"#,
        )
        .run_test("test/controllers/pages_controller_test.rb")
        .assert_passes();
}
