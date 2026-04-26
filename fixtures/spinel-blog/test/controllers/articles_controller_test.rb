require_relative "../test_helper"
require "models/article"
require "models/comment"

class ArticlesControllerTest < Minitest::Test
  include RequestDispatch

  def setup
    SchemaSetup.reset!
    @__session = {}
    @__flash = {}
    @article = Article.new(
      title: "Getting Started",
      body: "Rails is a web application framework running on Ruby.",
    )
    @article.save
  end

  # ── index ──────────────────────────────────────────────────────

  def test_index_responds_success
    res = get("/articles")
    assert res.success?
    assert_equal 200, res.status
  end

  def test_index_renders_h1
    res = get("/articles")
    assert_includes res.body, ">Articles</h1>"
  end

  def test_index_includes_existing_article
    res = get("/articles")
    assert_includes res.body, "Getting Started"
  end

  def test_index_orders_articles_descending_by_created_at
    sleep 0.01
    second = Article.new(title: "Second", body: "Some longer body content.")
    second.save
    res = get("/articles")
    pos_first  = res.body.index("Getting Started")
    pos_second = res.body.index("Second")
    refute_nil pos_first
    refute_nil pos_second
    assert pos_second < pos_first, "newer article should appear first"
  end

  # ── show ───────────────────────────────────────────────────────

  def test_show_responds_success
    res = get("/articles/#{@article.id}")
    assert res.success?
  end

  def test_show_renders_title_and_body
    res = get("/articles/#{@article.id}")
    assert_includes res.body, ">Getting Started</h1>"
    assert_includes res.body, "Rails is a web application framework"
  end

  def test_show_for_missing_article_raises
    assert_raises(ActiveRecord::RecordNotFound) { get("/articles/9999") }
  end

  # ── new ────────────────────────────────────────────────────────

  def test_new_renders_form
    res = get("/articles/new")
    assert res.success?
    assert_includes res.body, ">New article</h1>"
    assert_includes res.body, %(action="/articles")
  end

  # ── edit ───────────────────────────────────────────────────────

  def test_edit_renders_form_with_existing_values
    res = get("/articles/#{@article.id}/edit")
    assert res.success?
    assert_includes res.body, %(value="Getting Started")
    assert_includes res.body, %(<input type="hidden" name="_method" value="patch">)
  end

  # ── create ─────────────────────────────────────────────────────

  def test_create_with_valid_params_redirects_to_show
    res = post("/articles", params: { article: { title: "New post", body: "Body content with some length." } })
    assert res.redirect?
    new_id = Article.find_by(title: "New post").id
    assert_redirected_to "/articles/#{new_id}", res
    assert_equal "Article was successfully created.", res.flash[:notice]
  end

  def test_create_persists_record
    initial_count = Article.count
    post("/articles", params: { article: { title: "Persisted", body: "Body content with some length." } })
    assert_equal initial_count + 1, Article.count
  end

  def test_create_with_invalid_params_renders_new_with_422
    res = post("/articles", params: { article: { title: "", body: "short" } })
    assert res.unprocessable?
    assert_equal 422, res.status
    assert_includes res.body, ">New article</h1>"
    assert_includes res.body, %(id="error_explanation")
  end

  def test_create_with_invalid_params_does_not_persist
    initial_count = Article.count
    post("/articles", params: { article: { title: "", body: "short" } })
    assert_equal initial_count, Article.count
  end

  # ── update ─────────────────────────────────────────────────────

  def test_update_with_valid_params_redirects
    res = patch("/articles/#{@article.id}", params: { article: { title: "Renamed", body: @article.body } })
    assert res.redirect?
    assert_equal 303, res.status
    assert_redirected_to "/articles/#{@article.id}", res
  end

  def test_update_persists_changes
    patch("/articles/#{@article.id}", params: { article: { title: "Renamed", body: @article.body } })
    fresh = Article.find(@article.id)
    assert_equal "Renamed", fresh.title
  end

  def test_update_with_invalid_params_renders_edit_with_422
    res = patch("/articles/#{@article.id}", params: { article: { title: "", body: "short" } })
    assert res.unprocessable?
    assert_includes res.body, ">Editing article</h1>"
    assert_includes res.body, %(id="error_explanation")
  end

  # ── destroy ────────────────────────────────────────────────────

  def test_destroy_redirects_to_index
    res = delete("/articles/#{@article.id}")
    assert res.redirect?
    assert_equal 303, res.status
    assert_redirected_to "/articles", res
  end

  def test_destroy_removes_record
    delete("/articles/#{@article.id}")
    refute Article.exists?(@article.id)
  end

  def test_destroy_cascades_to_comments
    Comment.new(article_id: @article.id, commenter: "Alice", body: "Nice").save
    assert_equal 1, Comment.count
    delete("/articles/#{@article.id}")
    assert_equal 0, Comment.count
  end

  # ── routing ────────────────────────────────────────────────────

  def test_unknown_route_raises
    assert_raises(RuntimeError) { get("/nonexistent") }
  end
end
