require_relative "test_helper"

class ArticleTest < Minitest::Test
  def test_creates_an_article_with_valid_attributes
    article = articles(:one)
    refute_nil article.id
    assert_equal "Getting Started with Rails", article.title
  end

  def test_validates_title_presence
    article = Article.new(title: "", body: "Valid body content here.")
    refute article.save
  end

  def test_validates_body_minimum_length
    article = Article.new(title: "Valid Title", body: "Short")
    refute article.save
  end

  def test_destroys_comments_when_article_is_destroyed
    article = articles(:one)
    before = Comment.count
    article.destroy
    assert_equal before - 1, Comment.count
  end
end
