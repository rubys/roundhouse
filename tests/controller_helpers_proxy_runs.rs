//! campfire 8a6e429 reaches the controller's view context with
//! `helpers.dom_id(...)` inside an action, and its controller test reads
//! the response with `css_select(...).map { it["id"] }`. Both raised
//! NoMethodError on the emitted app while `check` was clean. (Kept out
//! of tests/emit_and_run.rs so concurrent appends there do not
//! conflict; same harness.)

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CONTROLLER: &str = r#"class ProbesController < ApplicationController
  def show
    article = Article.find(params[:id])
    render plain: [helpers.dom_id(article), helpers.dom_id(article, :edit), helpers.shout("hi")].join(" ")
  end
end
"#;

/// `helpers.<view helper>` and `helpers.<app helper>` both answer from
/// an action, as Rails' controller view context does.
#[test]
fn helpers_in_a_controller_action_reaches_view_and_app_helpers() {
    emit_and_run::real_blog()
        .write("app/helpers/shouts_helper.rb", "module ShoutsHelper\n  def shout(text)\n    text.upcase + \"!\"\n  end\nend\n")
        .write("app/controllers/probes_controller.rb", CONTROLLER)
        .edit(
            "config/routes.rb",
            "Rails.application.routes.draw do\n",
            "Rails.application.routes.draw do\n  get \"probes/:id\", to: \"probes#show\", as: :probe\n",
        )
        .write(
            "test/controllers/probes_controller_test.rb",
            r##"require "test_helper"

class ProbesControllerTest < ActionDispatch::IntegrationTest
  test "helpers answers dom_id and an app helper" do
    article = Article.create!(title: "Probe", body: "A body long enough to pass.")
    get "/probes/#{article.id}"
    assert_response :success
    assert_equal "article_#{article.id} edit_article_#{article.id} HI!", response.body
  end
end
"##,
        )
        .run_test("test/controllers/probes_controller_test.rb")
        .assert_passes();
}

/// `css_select` answers each matching element once, and `[]` reads its
/// attributes — the index renders one `id="article_N"` div per article.
#[test]
fn css_select_answers_each_element_once_with_its_attributes() {
    emit_and_run::real_blog()
        .write(
            "test/controllers/css_select_controller_test.rb",
            r##"require "test_helper"

class CssSelectControllerTest < ActionDispatch::IntegrationTest
  test "css_select reads the rendered elements" do
    a = Article.create!(title: "First", body: "A body long enough to pass.")
    b = Article.create!(title: "Second", body: "A body long enough to pass.")
    get "/articles"
    assert_response :success
    ids = css_select("div.flex[id]").map { it["id"] }
    assert_includes ids, "article_#{a.id}"
    assert_includes ids, "article_#{b.id}"
    assert_equal Article.count, ids.length
    assert_equal ids.uniq, ids
    assert_nil css_select("h1").first["id"]
  end
end
"##,
        )
        .run_test("test/controllers/css_select_controller_test.rb")
        .assert_passes();
}
