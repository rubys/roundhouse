require_relative "../test_helper"
require "models/article"
require "models/comment"
require "broadcasts"

class ArticleBroadcastsTest < Minitest::Test
  def setup
    SchemaSetup.reset!
    Broadcasts.reset_log!
  end

  # ── Article create/update/destroy ──────────────────────────────

  def test_create_emits_prepend_to_articles_stream
    article = Article.new(title: "First", body: "Some body content here.")
    article.save
    entries = Broadcasts.log.select { |e| e[:stream] == "articles" }
    assert_equal 1, entries.length
    assert_equal :prepend, entries.first[:action]
    assert_equal "articles", entries.first[:target]
    assert_includes entries.first[:html], "First"
  end

  def test_update_emits_replace_at_record_dom_id
    article = Article.new(title: "Initial", body: "Some body content here.")
    article.save
    Broadcasts.reset_log!
    article.update(title: "Renamed")
    entries = Broadcasts.log.select { |e| e[:stream] == "articles" }
    assert_equal 1, entries.length
    assert_equal :replace, entries.first[:action]
    assert_equal "article_#{article.id}", entries.first[:target]
    assert_includes entries.first[:html], "Renamed"
  end

  def test_destroy_emits_remove_at_record_dom_id
    article = Article.new(title: "Doomed", body: "Some body content here.")
    article.save
    article_id = article.id
    Broadcasts.reset_log!
    article.destroy
    article_remove = Broadcasts.log.find { |e| e[:stream] == "articles" && e[:action] == :remove }
    refute_nil article_remove
    assert_equal "article_#{article_id}", article_remove[:target]
    assert_equal "", article_remove[:html]
  end

  def test_failed_validation_emits_no_broadcast
    bad = Article.new(title: "", body: "short")
    refute bad.save
    assert_equal 0, Broadcasts.log.length
  end

  # ── Comment broadcasts (default per-article + parent re-render) ─

  def test_comment_create_emits_append_to_per_article_stream
    article = Article.new(title: "Host", body: "Some long body content here.")
    article.save
    Broadcasts.reset_log!
    comment = Comment.new(article_id: article.id, commenter: "Alice", body: "Nice")
    comment.save
    per_article = Broadcasts.log.select { |e| e[:stream] == "article_#{article.id}_comments" }
    assert_equal 1, per_article.length
    assert_equal :append, per_article.first[:action]
    assert_equal "comments", per_article.first[:target]
    assert_includes per_article.first[:html], "Alice"
  end

  def test_comment_create_replays_parent_article_partial
    article = Article.new(title: "Host", body: "Some long body content here.")
    article.save
    Broadcasts.reset_log!
    Comment.new(article_id: article.id, commenter: "Alice", body: "Nice").save
    on_articles_stream = Broadcasts.log.select { |e| e[:stream] == "articles" }
    assert_equal 1, on_articles_stream.length
    assert_equal :replace, on_articles_stream.first[:action]
    assert_equal "article_#{article.id}", on_articles_stream.first[:target]
    # Parent partial includes the (now 1) comment count.
    assert_includes on_articles_stream.first[:html], "(1 comment)"
  end

  def test_comment_destroy_emits_remove_and_replays_parent
    article = Article.new(title: "Host", body: "Some long body content here.")
    article.save
    comment = Comment.new(article_id: article.id, commenter: "Alice", body: "Nice")
    comment.save
    Broadcasts.reset_log!
    comment.destroy
    actions_per_stream = Broadcasts.log.group_by { |e| e[:stream] }
    assert_equal :remove, actions_per_stream["article_#{article.id}_comments"].first[:action]
    assert_equal :replace, actions_per_stream["articles"].first[:action]
  end

  def test_comment_destroy_after_parent_destroy_does_not_crash
    # When the article is destroyed, its before_destroy cascades to
    # comments. Each comment's after_destroy_commit then tries to
    # find the parent article. At the moment the comment's destroy
    # commit runs, the parent's row still exists in the DB (the
    # parent's adapter.delete runs *after* before_destroy completes),
    # so the parent re-render fires. This is consistent with real-blog
    # behavior — slightly redundant but not crashing.
    article = Article.new(title: "Host", body: "Some long body content here.")
    article.save
    Comment.new(article_id: article.id, commenter: "A", body: "B").save
    Comment.new(article_id: article.id, commenter: "C", body: "D").save
    Broadcasts.reset_log!
    article.destroy
    # No assertion on count — just confirming no exception was raised.
    refute Broadcasts.log.empty?
  end
end
