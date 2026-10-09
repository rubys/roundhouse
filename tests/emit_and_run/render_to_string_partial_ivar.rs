//! campfire's MessagesController#create renders a partial to a string
//! once, keeps it in an ivar, and hands that same markup to both the
//! broadcast and its own turbo_stream template.

use super::emit_and_run;

#[test]
fn a_partial_rendered_to_a_string_feeds_an_ivar_the_template_reads() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/previews/:id\", to: \"previews#show\"\n",
        )
        .write(
            "app/controllers/previews_controller.rb",
            r#"class PreviewsController < ApplicationController
  def show
    @article = Article.find(params[:id])
    @article_html = render_to_string partial: "articles/article", formats: :html, locals: { article: @article }
  end
end
"#,
        )
        .write("app/views/previews/show.html.erb", "<section><%= @article_html %></section>\n")
        .write(
            "test/controllers/previews_controller_test.rb",
            r#"require "test_helper"

class PreviewsControllerTest < ActionDispatch::IntegrationTest
  test "the string-rendered partial reaches the template once" do
    article = Article.create!(title: "Rendered once", body: "A body long enough.")
    get "/previews/#{article.id}"
    assert_response :success
    assert_includes response.body, "<section>"
    assert_includes response.body, "Rendered once"
    assert_equal 1, response.body.scan("Rendered once").size
  end
end
"#,
        )
        .run_test("test/controllers/previews_controller_test.rb")
        .assert_passes();
}
