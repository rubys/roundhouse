//! campfire's `ApplicationHelper#token_tag(*)` takes any arguments and
//! ignores them. An unforwarded anonymous rest binds to a generated
//! name, so the helper keeps its arity and runs from a view.

use super::emit_and_run;

#[test]
fn a_helper_with_an_unforwarded_anonymous_rest_runs_from_a_view() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/marks_helper.rb",
            "module MarksHelper\n  def mark(*)\n    \"[mark]\"\n  end\nend\n",
        )
        .edit(
            "app/views/articles/show.html.erb",
            "<% content_for :title, \"Showing article\" %>\n",
            "<% content_for :title, \"Showing article\" %>\n<p id=\"marks\"><%= mark %><%= mark(@article, :x, 3) %></p>\n",
        )
        .write(
            "test/controllers/marks_controller_test.rb",
            r#"require "test_helper"

class MarksControllerTest < ActionDispatch::IntegrationTest
  test "the helper answers with and without arguments" do
    article = Article.create!(title: "Marked", body: "A body long enough.")
    get "/articles/#{article.id}"
    assert_response :success
    assert_includes response.body, "[mark][mark]"
  end
end
"#,
        )
        .run_test("test/controllers/marks_controller_test.rb")
        .assert_passes();
}
