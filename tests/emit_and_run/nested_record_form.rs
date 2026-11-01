//! `form_with model: [parent, record]` where the record is a local, not
//! a `Class.new`: Rails' `polymorphic_path` names the nested helper from
//! every element (`article_comment_path(article, comment)`), picks the
//! member route with PATCH for a persisted record and the collection
//! route with POST for a new one, and names the fields after the record.
//! The array used to reach the plain-record lowering whole, which asked
//! the Array for `persisted?` and `id` and named the helper after the
//! view directory (`comment_path`), while `check` stayed clean.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/views/comments/_form.html.erb",
            "<%= form_with model: [article, comment], class: \"nested-record\" do |form| %>\n  <%= form.text_field :body %>\n<% end %>\n",
        )
        .edit(
            "app/views/articles/show.html.erb",
            "<h3 class=\"text-lg font-semibold mb-2\">Add a Comment</h3>\n",
            "<h3 class=\"text-lg font-semibold mb-2\">Add a Comment</h3>\n\n<%= render \"comments/form\", article: @article, comment: Comment.new(article: @article) %>\n<%= render \"comments/form\", article: @article, comment: @article.comments.first %>\n",
        )
}

const TEST: &str = r##"require "test_helper"

class NestedRecordFormsControllerTest < ActionDispatch::IntegrationTest
  test "a new record posts to the nested collection" do
    article = articles(:one)
    get article_url(article)
    assert_response :success
    assert_select "form.nested-record[action='/articles/#{article.id}/comments']", 1
    assert_select "form.nested-record[action='/articles/#{article.id}/comments'] input[name=_method]", 0
  end

  test "a persisted record patches its nested member" do
    article = articles(:one)
    comment = comments(:one)
    get article_url(article)
    assert_response :success
    assert_select "form.nested-record[action='/articles/#{article.id}/comments/#{comment.id}'] input[name=_method][value=patch]", 1
    assert_includes response.body, "name=\"comment[body]\" id=\"comment_body\" value=\"#{comment.body}\""
  end
end
"##;

#[test]
fn a_nested_record_form_targets_the_nested_member_or_collection() {
    app()
        .write("test/controllers/nested_record_forms_controller_test.rb", TEST)
        .run_test("test/controllers/nested_record_forms_controller_test.rb")
        .assert_passes();
}

const SPINEL: &str = r##"article = Article.create!(title: "Nested", body: "A body long enough to validate.")
comment = Comment.create!(article: article, commenter: "Alice", body: "Saved")
fresh = Views::Comments.form(Comment.new(article: article), nil, nil, article)
raise fresh unless fresh.include?("action=\"/articles/#{article.id}/comments\"")
raise fresh if fresh.include?("name=\"_method\"")
saved = Views::Comments.form(comment, nil, nil, article)
raise saved unless saved.include?("action=\"/articles/#{article.id}/comments/#{comment.id}\"")
raise saved unless saved.include?("name=\"_method\" value=\"patch\"")
raise saved unless saved.include?("name=\"comment[body]\"")
puts "nested record form passed"
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_nested_record_form_targets_the_nested_member_or_collection_on_spinel() {
    app().run_spinel_with_db(SPINEL).assert_passes();
}
