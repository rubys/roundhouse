//! `render json:` of `record.errors.full_messages` writes the Array of
//! full messages, as Rails does, bare or inside a Hash. The
//! `full_messages` hop folds to the `errors` accumulator, and the
//! render-json rewrite then read that as `render json: record.errors`
//! and wrote the errors keyed by attribute (`errors_json`, #586). Its
//! oracle:
//!
//!     ruby tests/render_json_full_messages/oracle.rb

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

#[test]
fn render_json_full_messages_is_the_array_of_messages() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/drafts/check\", to: \"drafts#check\"\n",
        )
        .write(
            "app/controllers/drafts_controller.rb",
            r#"class DraftsController < ApplicationController
  def check
    article = Article.new(title: "", body: "x")
    if params[:as] == "hash"
      article.valid?
      render json: { errors: article.errors.full_messages }
    elsif params[:as] == "keyed"
      article.valid?
      render json: { errors: article.errors }
    else
      render json: [article.valid?, article.errors.full_messages]
    end
  end
end
"#,
        )
        .write(
            "test/controllers/drafts_controller_test.rb",
            r#"require "test_helper"

class DraftsControllerTest < ActionDispatch::IntegrationTest
  test "full messages, bare and inside a hash" do
    get "/drafts/check"
    assert_equal %([false,["Title can't be blank","Body is too short (minimum is 10 characters)"]]), response.body
    get "/drafts/check", params: { as: "hash" }
    assert_equal %({"errors":["Title can't be blank","Body is too short (minimum is 10 characters)"]}), response.body
  end

  test "the errors themselves stay keyed by attribute" do
    get "/drafts/check", params: { as: "keyed" }
    assert_equal %({"errors":{"title":["can't be blank"],"body":["is too short (minimum is 10 characters)"]}}), response.body
  end
end
"#,
        )
        .run_test("test/controllers/drafts_controller_test.rb")
        .assert_passes();
}
