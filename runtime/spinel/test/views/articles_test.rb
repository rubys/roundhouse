require_relative "../test_helper"
require "models/article"
require "models/comment"
require "views"

class ViewsArticlesTest < Minitest::Test
  # Bring `ActionView::ViewHelpers` into scope as bare `ViewHelpers`
  # — matches Ruby's `include` idiom for nested-module access.
  include ActionView

  def setup
    SchemaSetup.reset!
    ViewHelpers.reset_slots!
    @article = Article.new(
      title: "Getting Started",
      body: "Rails is a web application framework running on Ruby.",
    )
    @article.save
  end

  # ── partial: _article.rb ────────────────────────────────────────

  def test_article_partial_includes_dom_id
    html = Views::Articles.article(@article)
    assert_includes html, %(id="article_#{@article.id}")
  end

  def test_article_partial_links_to_show
    html = Views::Articles.article(@article)
    assert_includes html, %(href="/articles/#{@article.id}")
  end

  def test_article_partial_links_to_edit
    html = Views::Articles.article(@article)
    assert_includes html, %(href="/articles/#{@article.id}/edit")
  end

  def test_article_partial_destroy_button
    html = Views::Articles.article(@article)
    assert_includes html, %(<input type="hidden" name="_method" value="delete">)
    assert_includes html, %(data-turbo-confirm="Are you sure?")
  end

  def test_article_partial_shows_comment_count_pluralized
    Comment.new(article_id: @article.id, commenter: "A", body: "B").save
    Comment.new(article_id: @article.id, commenter: "C", body: "D").save
    html = Views::Articles.article(@article)
    assert_includes html, "(2 comments)"
  end

  def test_article_partial_shows_zero_comments
    html = Views::Articles.article(@article)
    assert_includes html, "(0 comments)"
  end

  def test_article_partial_truncates_long_body
    long = "x" * 200
    art = Article.new(title: "T", body: long)
    art.save
    html = Views::Articles.article(art)
    # Truncated to 100 chars total (97 chars + "...").
    assert_includes html, "x" * 97 + "..."
  end

  # ── index.rb ────────────────────────────────────────────────────

  def test_index_renders_h1_articles
    html = Views::Articles.index([@article])
    assert_includes html, "<h1"
    assert_includes html, ">Articles</h1>"
  end

  def test_index_links_to_new_article
    html = Views::Articles.index([@article])
    assert_includes html, %(href="/articles/new")
    assert_includes html, ">New article</a>"
  end

  def test_index_renders_article_partials
    article2 = Article.new(title: "Second", body: "Long enough body content here.")
    article2.save
    html = Views::Articles.index([@article, article2])
    assert_includes html, %(id="article_#{@article.id}")
    assert_includes html, %(id="article_#{article2.id}")
  end

  def test_index_empty_state
    html = Views::Articles.index([])
    assert_includes html, "No articles found."
  end

  def test_index_displays_notice_when_provided
    # The lowered view uses positional `notice = nil` (Param dialect
    # doesn't carry keyword variants today; see
    # `project_lowered_ir_gaps_for_runnability`). Rails-idiom
    # `notice: "x"` would land as a Hash bound to the `notice` slot,
    # escaping to `{notice: "x"}` in the rendered HTML.
    html = Views::Articles.index([@article], "Article saved")
    assert_includes html, %(id="notice")
    assert_includes html, ">Article saved</p>"
  end

  def test_index_omits_notice_when_nil
    html = Views::Articles.index([@article])
    refute_includes html, %(id="notice")
  end

  def test_index_subscribes_to_articles_stream
    html = Views::Articles.index([@article])
    assert_includes html, %(<turbo-cable-stream-source)
    # base64(JSON("articles")) — matches Rails' signed-stream-name shape
    # after the compare harness strips the `--<sig>` suffix.
    assert_includes html, %(signed-stream-name="ImFydGljbGVzIg==--unsigned")
  end

  # ── show.rb ─────────────────────────────────────────────────────

  def test_show_renders_title
    html = Views::Articles.show(@article)
    assert_includes html, ">Getting Started</h1>"
  end

  def test_show_renders_body
    html = Views::Articles.show(@article)
    assert_includes html, "Rails is a web application framework"
  end

  def test_show_links_to_edit_and_back
    html = Views::Articles.show(@article)
    assert_includes html, %(href="/articles/#{@article.id}/edit")
    assert_includes html, %(href="/articles")
  end

  def test_show_includes_destroy_button
    html = Views::Articles.show(@article)
    assert_includes html, %(<input type="hidden" name="_method" value="delete">)
  end

  def test_show_subscribes_to_comments_stream
    # Use the fixture article (id=1) so the asserted base64 stream-name
    # is stable. Setup's `@article` saves on top of the loaded fixtures
    # and ends up at id=3, which would shift the encoded stream-name.
    fixture_article = ArticlesFixtures.one
    html = Views::Articles.show(fixture_article)
    # base64(JSON("article_1_comments")) → ImFydGljbGVfMV9jb21tZW50cyI=
    assert_includes html, %(signed-stream-name="ImFydGljbGVfMV9jb21tZW50cyI=--unsigned")
  end

  def test_show_renders_comments
    Comment.new(article_id: @article.id, commenter: "Alice", body: "Nice").save
    html = Views::Articles.show(@article)
    assert_includes html, ">Alice</p>"
    assert_includes html, ">Nice</p>"
  end

  def test_show_renders_new_comment_form
    html = Views::Articles.show(@article)
    assert_includes html, %(action="/articles/#{@article.id}/comments")
    assert_includes html, %(name="comment[commenter]")
    assert_includes html, %(name="comment[body]")
  end

  # ── new.rb ──────────────────────────────────────────────────────

  def test_new_renders_form_with_post_action
    article = Article.new
    html = Views::Articles.new(article)
    assert_includes html, ">New article</h1>"
    assert_includes html, %(action="/articles")
    assert_includes html, %(method="post")
    refute_includes html, "_method"
  end

  def test_new_links_back_to_index
    html = Views::Articles.new(Article.new)
    assert_includes html, %(href="/articles")
    assert_includes html, ">Back to articles</a>"
  end

  # ── edit.rb ─────────────────────────────────────────────────────

  def test_edit_renders_form_with_patch_method
    html = Views::Articles.edit(@article)
    assert_includes html, ">Editing article</h1>"
    assert_includes html, %(action="/articles/#{@article.id}")
    assert_includes html, %(<input type="hidden" name="_method" value="patch">)
  end

  def test_edit_form_prefills_values
    html = Views::Articles.edit(@article)
    assert_includes html, %(value="Getting Started")
  end

  # ── _form.rb partial ────────────────────────────────────────────

  def test_form_displays_validation_errors
    bad = Article.new(title: "")
    refute bad.save  # populates errors
    html = Views::Articles.form(bad)
    assert_includes html, %(id="error_explanation")
    # html_escape converts the apostrophe to &#39; — assert on the
    # escaped form (the browser un-escapes it for display).
    assert_includes html, "title can&#39;t be blank"
  end

  def test_form_omits_error_section_when_no_errors
    article = Article.new
    html = Views::Articles.form(article)
    refute_includes html, %(id="error_explanation")
  end

  # ── layouts/application.rb ──────────────────────────────────────

  def test_layout_wraps_body
    body = "<p>hello</p>"
    html = Views::Layouts.application(body)
    assert_includes html, "<!DOCTYPE html>"
    assert_includes html, "<html>"
    assert_includes html, "<body>"
    assert_includes html, "<p>hello</p>"
    assert_includes html, "</html>"
  end

  def test_layout_uses_content_for_title
    ViewHelpers.content_for_set(:title, "My Page")
    html = Views::Layouts.application("body")
    assert_includes html, "<title>My Page</title>"
  end

  def test_layout_falls_back_to_default_title
    html = Views::Layouts.application("body")
    assert_includes html, "<title>Real Blog</title>"
  end

  def test_layout_includes_csrf_and_stylesheet
    html = Views::Layouts.application("body")
    assert_includes html, %(name="csrf-token")
    assert_includes html, %(rel="stylesheet")
  end
end
