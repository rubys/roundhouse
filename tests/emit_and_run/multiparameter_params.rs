//! Multiparameter keys (`attr(N)`) on real-blog's articles. Rails'
//! `permit(:title)` keeps `title(1)`, and assignment gives a String or
//! Text attribute the text of the parts Hash — `{1 => "short"}` (Ruby
//! 3.4's Hash#inspect) — which validations then see and a save writes.
//! The expected values are Rails 8.2.0.alpha's on Ruby 4.0.5 for the
//! same requests. A shape or attribute whose Rails value is not
//! reproduced is refused (a RuntimeError naming the gap) before the record
//! changes.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.string \"title\"\n",
            "    t.string \"title\"\n    t.date \"published_on\"\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "params.expect(article: [ :title, :body ])",
            "params.expect(article: [ :title, :body, :published_on ])",
        )
}

const TEST: &str = r#"require "test_helper"

class MultiparameterControllerTest < ActionDispatch::IntegrationTest
  test "an update stores the cast of the parts, which validations see" do
    article = articles(:one)
    patch article_url(article), params: { article: { "body(1)" => "short" } }
    assert_redirected_to article_url(article)
    assert_equal "{1 => \"short\"}", article.reload.body
  end

  test "a create stores the cast of the parts" do
    assert_difference("Article.count") do
      post articles_url, params: { article: { "title(1)" => "T", body: "long enough body" } }
    end
    assert_equal "{1 => \"T\"}", Article.last.title
  end

  test "integer parts, and a blank part, are cast as Rails casts them" do
    article = articles(:one)
    patch article_url(article), params: { article: { "body(1i)" => "2026", "body(2)" => "", "body(3)" => "x y" } }
    assert_equal "{1 => 2026, 2 => nil, 3 => \"x y\"}", article.reload.body
  end

  test "a part Hash#inspect would escape is refused before anything changes" do
    article = articles(:one)
    assert_raises(RuntimeError) do
      patch article_url(article), params: { article: { "body(1)" => "say \"hi\" here" } }
    end
    assert_equal "Rails is a web application framework running on the Ruby programming language.", article.reload.body
  end

  test "a date attribute's parts are refused, not stored as text" do
    article = articles(:one)
    assert_raises(RuntimeError) do
      patch article_url(article), params: { article: { "published_on(1i)" => "2026", "published_on(2i)" => "10", "published_on(3i)" => "25" } }
    end
    assert_nil article.reload.published_on
  end
end
"#;

#[test]
fn multiparameter_keys_store_what_rails_stores_or_refuse() {
    app()
        .write("test/controllers/multiparameter_controller_test.rb", TEST)
        .run_test("test/controllers/multiparameter_controller_test.rb")
        .assert_passes();
}
