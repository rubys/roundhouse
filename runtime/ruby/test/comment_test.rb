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

  # NOTE: `requires valid article` isn't enforced. Matches Juntos's
  # baseline — `validates_presence_of(:article)` only checks the
  # `article_id` FK is set, not that it references an existing Article.
  # Rails's belongs_to adds this check as a separate concern (via
  # validates_associated-like behavior) and versions vary; leaving it
  # to the runtime keeps our framework behavior in parity with Juntos.
end
