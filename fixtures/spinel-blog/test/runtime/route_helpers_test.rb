require_relative "../test_helper"
require "action_view"

class RouteHelpersTest < Minitest::Test
  def test_articles_path
    assert_equal "/articles", RouteHelpers.articles_path
  end

  def test_article_path
    assert_equal "/articles/42", RouteHelpers.article_path(42)
  end

  def test_new_article_path
    assert_equal "/articles/new", RouteHelpers.new_article_path
  end

  def test_edit_article_path
    assert_equal "/articles/42/edit", RouteHelpers.edit_article_path(42)
  end

  def test_article_comments_path
    assert_equal "/articles/42/comments", RouteHelpers.article_comments_path(42)
  end

  def test_article_comment_path
    assert_equal "/articles/42/comments/7", RouteHelpers.article_comment_path(42, 7)
  end

  def test_root_path
    assert_equal "/", RouteHelpers.root_path
  end
end
