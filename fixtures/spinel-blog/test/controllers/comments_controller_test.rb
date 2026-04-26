require_relative "../test_helper"
require "models/article"
require "models/comment"

class CommentsControllerTest < Minitest::Test
  include RequestDispatch

  def setup
    SchemaSetup.reset!
    @__session = {}
    @__flash = {}
    @article = Article.new(
      title: "Host article",
      body: "Body long enough to satisfy minimum-length validation.",
    )
    @article.save
  end

  # ── create ─────────────────────────────────────────────────────

  def test_create_with_valid_params_redirects_to_article
    res = post("/articles/#{@article.id}/comments",
               params: { comment: { commenter: "Alice", body: "Nice post" } })
    assert res.redirect?
    assert_redirected_to "/articles/#{@article.id}", res
    assert_equal "Comment was successfully created.", res.flash[:notice]
  end

  def test_create_persists_comment_with_correct_fk
    post("/articles/#{@article.id}/comments",
         params: { comment: { commenter: "Alice", body: "Nice post" } })
    comment = Comment.where(article_id: @article.id).first
    refute_nil comment
    assert_equal "Alice", comment.commenter
    assert_equal "Nice post", comment.body
    assert_equal @article.id, comment.article_id
  end

  def test_create_with_invalid_params_redirects_with_alert
    res = post("/articles/#{@article.id}/comments",
               params: { comment: { commenter: "", body: "" } })
    assert res.redirect?
    assert_redirected_to "/articles/#{@article.id}", res
    assert_equal "Could not create comment.", res.flash[:alert]
  end

  def test_create_with_invalid_params_does_not_persist
    initial_count = Comment.count
    post("/articles/#{@article.id}/comments",
         params: { comment: { commenter: "", body: "" } })
    assert_equal initial_count, Comment.count
  end

  # ── destroy ────────────────────────────────────────────────────

  def test_destroy_removes_comment_and_redirects
    comment = Comment.new(article_id: @article.id, commenter: "Alice", body: "Nice")
    comment.save
    res = delete("/articles/#{@article.id}/comments/#{comment.id}")
    assert res.redirect?
    assert_redirected_to "/articles/#{@article.id}", res
    refute Comment.exists?(comment.id)
  end

  def test_destroy_404s_when_comment_belongs_to_other_article
    other = Article.new(title: "Other", body: "Other body content here.")
    other.save
    foreign = Comment.new(article_id: other.id, commenter: "Eve", body: "Hi")
    foreign.save
    res = delete("/articles/#{@article.id}/comments/#{foreign.id}")
    assert_equal 404, res.status
    assert Comment.exists?(foreign.id)
  end

  def test_destroy_sets_flash_notice
    comment = Comment.new(article_id: @article.id, commenter: "Alice", body: "Nice")
    comment.save
    res = delete("/articles/#{@article.id}/comments/#{comment.id}")
    assert_equal "Comment was successfully deleted.", res.flash[:notice]
  end
end
