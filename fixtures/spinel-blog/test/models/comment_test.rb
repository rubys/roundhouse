require_relative "../test_helper"
require "models/article"
require "models/comment"

class CommentTest < Minitest::Test
  def setup
    SchemaSetup.reset!
    @article = Article.new(
      title: "Host article",
      body: "Body long enough to satisfy minimum-length validation.",
    )
    @article.save
  end

  def test_save_with_valid_attributes
    comment = Comment.new(article_id: @article.id, commenter: "Alice", body: "Nice")
    assert comment.save
    assert comment.persisted?
    assert comment.id > 0
  end

  def test_validates_commenter_presence
    comment = Comment.new(article_id: @article.id, commenter: "", body: "Body")
    refute comment.save
    assert_includes comment.errors, "commenter can't be blank"
  end

  def test_validates_body_presence
    comment = Comment.new(article_id: @article.id, commenter: "Alice", body: nil)
    refute comment.save
    assert_includes comment.errors, "body can't be blank"
  end

  def test_belongs_to_article_returns_owner
    comment = Comment.new(article_id: @article.id, commenter: "Alice", body: "Nice")
    comment.save
    assert_equal @article.id, comment.article.id
  end

  def test_belongs_to_returns_nil_when_fk_unset
    comment = Comment.new(commenter: "Anon", body: "Body")
    assert_nil comment.article
  end

  def test_belongs_to_returns_nil_when_fk_missing_in_db
    comment = Comment.new(article_id: 99_999, commenter: "Alice", body: "Body")
    comment.save
    assert_nil comment.article
  end

  def test_destroy_removes_comment_only
    c = Comment.new(article_id: @article.id, commenter: "Alice", body: "Nice")
    c.save
    c.destroy
    assert_equal 0, Comment.count
    assert Article.exists?(@article.id)
  end
end
