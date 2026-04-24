require_relative "test_helper"

class CommentTest < Minitest::Test
  def test_creates_a_comment_on_an_article
    comment = comments(:one)
    refute_nil comment.id
    assert_equal articles(:one).id, comment.article_id
  end

  def test_belongs_to_article_association
    article = articles(:one)
    comment = article.comments.create(commenter: "Commenter", body: "Comment body text.")
    assert_equal article.id, comment.article_id
  end

  def test_requires_commenter
    article = articles(:one)
    comment = article.comments.build(body: "Comment without commenter")
    refute comment.save
  end

  def test_requires_body
    article = articles(:one)
    comment = article.comments.build(commenter: "Someone")
    refute comment.save
  end

  def test_requires_valid_article
    comment = Comment.new(commenter: "Test", body: "A test comment.", article_id: 999999)
    refute comment.save
  end
end
