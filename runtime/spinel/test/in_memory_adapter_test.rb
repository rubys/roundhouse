require_relative "test_helper"
require "in_memory_adapter"
require "models/article"
require "models/comment"

# Tests that InMemoryAdapter satisfies the same contract SqliteAdapter
# does, exercised via the Article/Comment models. Each test swaps
# ActiveRecord.adapter to InMemoryAdapter for the duration, then
# restores the SqliteAdapter so other tests in the suite are
# unaffected.
class InMemoryAdapterTest < Minitest::Test
  def setup
    @prior_adapter = ActiveRecord.adapter
    InMemoryAdapter.configure
    ActiveRecord.adapter = InMemoryAdapter
    Broadcasts.reset_log!
  end

  def teardown
    ActiveRecord.adapter = @prior_adapter
  end

  # ── direct adapter API ──────────────────────────────────────────

  def test_insert_returns_assigned_id
    id1 = InMemoryAdapter.insert("articles", { title: "A", body: "x" })
    id2 = InMemoryAdapter.insert("articles", { title: "B", body: "y" })
    assert_equal 1, id1
    assert_equal 2, id2
  end

  def test_insert_stores_attributes
    id = InMemoryAdapter.insert("articles", { title: "Hello", body: "World" })
    row = InMemoryAdapter.find("articles", id)
    assert_equal "Hello", row[:title]
    assert_equal "World", row[:body]
    assert_equal id, row[:id]
  end

  def test_all_returns_inserted_rows
    InMemoryAdapter.insert("articles", { title: "A" })
    InMemoryAdapter.insert("articles", { title: "B" })
    titles = InMemoryAdapter.all("articles").map { |r| r[:title] }
    assert_equal ["A", "B"], titles.sort
  end

  def test_where_filters_by_equality
    InMemoryAdapter.insert("articles", { title: "Hit", body: "x" })
    InMemoryAdapter.insert("articles", { title: "Miss", body: "y" })
    rows = InMemoryAdapter.where("articles", title: "Hit")
    assert_equal 1, rows.length
    assert_equal "Hit", rows.first[:title]
  end

  def test_where_with_multiple_conditions_is_AND
    InMemoryAdapter.insert("comments", { article_id: 1, commenter: "Alice" })
    InMemoryAdapter.insert("comments", { article_id: 1, commenter: "Bob" })
    InMemoryAdapter.insert("comments", { article_id: 2, commenter: "Alice" })
    rows = InMemoryAdapter.where("comments", article_id: 1, commenter: "Alice")
    assert_equal 1, rows.length
  end

  def test_count_reports_table_size
    3.times { |i| InMemoryAdapter.insert("articles", { title: "t#{i}" }) }
    assert_equal 3, InMemoryAdapter.count("articles")
  end

  def test_exists_predicate
    id = InMemoryAdapter.insert("articles", { title: "x" })
    assert InMemoryAdapter.exists?("articles", id)
    refute InMemoryAdapter.exists?("articles", 9999)
  end

  def test_update_modifies_attributes_in_place
    id = InMemoryAdapter.insert("articles", { title: "Old" })
    InMemoryAdapter.update("articles", id, { title: "New" })
    assert_equal "New", InMemoryAdapter.find("articles", id)[:title]
  end

  def test_delete_removes_row
    id = InMemoryAdapter.insert("articles", { title: "x" })
    InMemoryAdapter.delete("articles", id)
    refute InMemoryAdapter.exists?("articles", id)
  end

  def test_truncate_clears_table_and_resets_id
    InMemoryAdapter.insert("articles", { title: "x" })
    InMemoryAdapter.insert("articles", { title: "y" })
    InMemoryAdapter.truncate("articles")
    assert_equal 0, InMemoryAdapter.count("articles")
    fresh_id = InMemoryAdapter.insert("articles", { title: "z" })
    assert_equal 1, fresh_id, "next_id should reset on truncate"
  end

  def test_tables_are_isolated
    InMemoryAdapter.insert("articles", { title: "x" })
    InMemoryAdapter.insert("comments", { commenter: "y" })
    assert_equal 1, InMemoryAdapter.count("articles")
    assert_equal 1, InMemoryAdapter.count("comments")
  end

  def test_execute_ddl_records_table_name
    InMemoryAdapter.execute_ddl("CREATE TABLE IF NOT EXISTS posts (id INTEGER)")
    assert_equal 0, InMemoryAdapter.count("posts")
  end

  def test_execute_ddl_index_is_noop
    # CREATE INDEX shouldn't crash, just no-op.
    InMemoryAdapter.execute_ddl("CREATE INDEX foo ON articles (title)")
  end

  # ── exercised through the model layer ───────────────────────────

  def test_article_save_through_in_memory_adapter
    article = Article.new(title: "From InMem", body: "Long enough body content here.")
    assert article.save
    assert article.persisted?
    assert article.id > 0
  end

  def test_article_validation_errors_via_in_memory
    bad = Article.new(title: "", body: "short")
    refute bad.save
    assert_includes bad.errors, "title can't be blank"
  end

  def test_article_find_returns_typed_instance
    a = Article.new(title: "Findable", body: "Long enough body content here.")
    a.save
    fresh = Article.find(a.id)
    assert_equal "Findable", fresh.title
    assert_kind_of Article, fresh
  end

  def test_article_destroy_cascades_to_comments
    article = Article.new(title: "Host", body: "Long enough body content here.")
    article.save
    Comment.new(article_id: article.id, commenter: "Alice", body: "Hi").save
    Comment.new(article_id: article.id, commenter: "Bob",   body: "Hey").save
    assert_equal 2, Comment.count
    article.destroy
    assert_equal 0, Comment.count
  end

  def test_comment_belongs_to_article_resolves
    article = Article.new(title: "Host", body: "Long enough body content here.")
    article.save
    comment = Comment.new(article_id: article.id, commenter: "A", body: "B")
    comment.save
    assert_equal article.id, comment.article.id
  end

  def test_save_emits_broadcast
    article = Article.new(title: "Broadcast", body: "Long enough body content here.")
    article.save
    entry = Broadcasts.log.find { |e| e[:stream] == "articles" }
    refute_nil entry
    assert_equal :prepend, entry[:action]
  end
end
