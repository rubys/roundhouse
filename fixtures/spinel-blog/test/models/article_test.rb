require_relative "../test_helper"
require "models/article"
require "models/comment"

class ArticleTest < Minitest::Test
  def setup
    SchemaSetup.reset!
    @article = Article.new(
      title: "Getting Started with Rails",
      body: "Rails is a web application framework running on Ruby.",
      created_at: "2026-04-26T00:00:00Z",
      updated_at: "2026-04-26T00:00:00Z",
    )
    @article.save
  end

  def test_save_persists_with_assigned_id
    assert @article.persisted?
    assert @article.id > 0
  end

  def test_initial_attributes_round_trip
    assert_equal "Getting Started with Rails", @article.title
    assert @article.body.start_with?("Rails is a")
  end

  def test_validates_title_presence
    bad = Article.new(title: "", body: "Valid body content here.")
    refute bad.save
    assert_includes bad.errors, "title can't be blank"
  end

  def test_validates_body_presence
    bad = Article.new(title: "Some title", body: nil)
    refute bad.save
    assert_includes bad.errors, "body can't be blank"
  end

  def test_validates_body_minimum_length
    bad = Article.new(title: "Valid Title", body: "Short")
    refute bad.save
    assert_includes bad.errors, "body is too short (minimum is 10)"
  end

  def test_find_by_id
    found = Article.find(@article.id)
    assert_equal @article.id, found.id
    assert_equal @article.title, found.title
  end

  def test_find_raises_when_missing
    assert_raises(ActiveRecord::RecordNotFound) { Article.find(99_999) }
  end

  def test_find_by_with_conditions
    found = Article.find_by(title: "Getting Started with Rails")
    refute_nil found
    assert_equal @article.id, found.id
  end

  def test_find_by_returns_nil_when_no_match
    assert_nil Article.find_by(title: "No such article")
  end

  def test_where_filters
    Article.new(title: "Other", body: "Different content here.").save
    matches = Article.where(title: "Getting Started with Rails")
    assert_equal 1, matches.length
    assert_equal @article.id, matches.first.id
  end

  def test_count
    Article.new(title: "Second", body: "Second body content.").save
    assert_equal 2, Article.count
  end

  def test_exists
    assert Article.exists?(@article.id)
    refute Article.exists?(99_999)
  end

  def test_update_changes_persisted_value
    ok = @article.update(title: "Renamed")
    assert ok
    fresh = Article.find(@article.id)
    assert_equal "Renamed", fresh.title
  end

  def test_update_runs_validations
    ok = @article.update(title: "")
    refute ok
    assert_includes @article.errors, "title can't be blank"
  end

  def test_destroy_removes_record
    id = @article.id
    @article.destroy
    assert @article.destroyed?
    refute @article.persisted?
    refute Article.exists?(id)
  end

  def test_destroys_comments_when_article_is_destroyed
    Comment.new(article_id: @article.id, commenter: "Alice", body: "Nice!").save
    assert_equal 1, Comment.count
    @article.destroy
    assert_equal 0, Comment.count
  end

  def test_comments_association_returns_scoped_array
    Comment.new(article_id: @article.id, commenter: "Alice", body: "Hi").save
    Comment.new(article_id: @article.id, commenter: "Bob",   body: "Hi").save
    other = Article.new(title: "Other", body: "Other body content.")
    other.save
    Comment.new(article_id: other.id, commenter: "Eve", body: "Hi").save
    list = @article.comments
    assert_equal 2, list.length
    list.each { |c| assert_equal @article.id, c.article_id }
  end

  def test_destroy_all_clears_table
    Article.new(title: "Second", body: "Second body content.").save
    Article.destroy_all
    assert_equal 0, Article.count
  end
end
