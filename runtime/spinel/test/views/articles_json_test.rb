# Minitest-shaped: this is a CRuby-only framework test, quarantined
# out of the spin shape by `project.rs::spin_shape` (it is not a spin
# test program and gets no snapshot). It therefore loads its own
# runner — `test_helper` deliberately does not, because the EMITTED
# tests inherit `TestBase` and carry their own driver shim.
require "minitest/autorun"
require_relative "../test_helper"
require "models/article"
require "models/comment"
require "views"

# Phase 4 acceptance for the Jbuilder lowerer pipeline. Exercises the
# three `Views::Articles.<x>_json` methods the lowerer emits from
# `fixtures/real-blog/app/views/articles/*.json.jbuilder`. Asserts on
# substring matches over the lowered JSON output rather than parsing
# (the spinel-subset `runtime/json.rb` shim only emits `JSON.generate`;
# no `parse` companion). `article_url` is absolute on the test host,
# format suffix included (`http://www.example.com/articles/N.json`).

class ViewsArticlesJsonTest < Minitest::Test
  def setup
    SchemaSetup.reset!
    # A direct view render has no request. The integration harness
    # parks Rails' test host here (TestBase#setup / host!); do the
    # same so `url_for_path` answers the absolute URL Rails writes.
    ActionView::ViewHelpers.url_origin = "http://www.example.com"
    @article = Article.new(
      title: "Getting Started",
      body: "Rails is a web application framework running on Ruby.",
    )
    @article.save
  end

  # ── partial: _article_json.rb ───────────────────────────────────

  def test_article_json_partial_opens_and_closes_object
    j = Views::Articles.article_json(@article)
    assert j.start_with?("{"), "expected `{` opener: #{j}"
    assert j.end_with?("}"), "expected `}` closer: #{j}"
  end

  def test_article_json_partial_includes_extracted_fields
    j = Views::Articles.article_json(@article)
    assert_includes j, %("id":#{@article.id})
    assert_includes j, %("title":"Getting Started")
    assert_includes j, %("body":"Rails is a web application framework running on Ruby.")
    # datetime columns format as Rails-canonical ISO 8601 with
    # millisecond precision + Z suffix — `JsonBuilder.encode_datetime`
    # reformats the sqlite TEXT shape ("2026-05-10 02:22:28.114670")
    # to the ActiveSupport::JSON::Encoding default
    # ("2026-05-10T02:22:28.114Z").
    assert_match(/"created_at":"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z"/, j)
    assert_match(/"updated_at":"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z"/, j)
  end

  def test_article_json_partial_url_is_absolute
    j = Views::Articles.article_json(@article)
    # `json.url article_url(article, format: :json)` lowers to
    # `ViewHelpers.url_for_path(RouteHelpers.article_path(id) + ".json")`.
    # Rails answers an absolute URL on the test host, format suffix included.
    assert_includes j, %("url":"http://www.example.com/articles/#{@article.id}.json")
  end

  def test_article_json_escapes_special_characters
    a = Article.new(title: %(He said "hi"), body: "back\\slash\nnewline")
    a.save
    j = Views::Articles.article_json(a)
    # Quotes inside the title are backslash-escaped; the encoded
    # fragment is `"He said \"hi\""`. In Ruby source that's
    # `"\"He said \\\"hi\\\"\""`.
    assert_includes j, "\"title\":\"He said \\\"hi\\\"\""
    # `\\` and `\n` in source survive as `\\\\` and `\\n` in JSON.
    assert_includes j, "\"body\":\"back\\\\slash\\nnewline\""
  end

  # ── show: show_json.rb ──────────────────────────────────────────

  def test_show_json_renders_single_article_as_object
    j = Views::Articles.show_json(@article)
    assert j.start_with?("{") && j.end_with?("}"), j
    assert_includes j, %("id":#{@article.id})
    assert_includes j, %("title":"Getting Started")
  end

  # ── index: index_json.rb ────────────────────────────────────────

  def test_index_json_renders_empty_array_for_empty_collection
    Article._adapter_truncate
    assert_equal "[]", Views::Articles.index_json([])
  end

  def test_index_json_renders_single_element_array
    j = Views::Articles.index_json([@article])
    assert j.start_with?("[") && j.end_with?("]"), j
    assert_includes j, %("id":#{@article.id})
    refute_includes j, ",]", "trailing comma: #{j}"
  end

  def test_index_json_renders_multi_element_array_with_comma_separator
    other = Article.new(title: "Second", body: "Two")
    other.save
    j = Views::Articles.index_json([@article, other])
    assert_includes j, "Getting Started"
    assert_includes j, "Second"
    assert_includes j, "},{", "missing comma between elements: #{j}"
    refute_includes j, ",]", "trailing comma: #{j}"
  end

  # ── controller dispatch end-to-end (head + location kwargs) ─────

  # test_helper.rb sets `Db.configure(":memory:")` + schema + fixtures
  # at load time, BEFORE main.rb is required. We deliberately skip
  # `Main.configure_default_adapter!` here — it would call
  # `Db.configure(":memory:")` again, opening a fresh empty DB
  # connection and blowing away the setUp fixtures + saved @article.
  # ActiveRecord.adapter stays nil per test_helper's policy (any path
  # that falls through surfaces a NoMethodError; the lowered
  # controllers use Level-3 Db.exec, not the adapter dispatcher).
  def test_delete_articles_json_returns_204_without_content_type
    require_relative "../../main"
    out = StringIO.new
    Main.run(
      { "REQUEST_METHOD" => "DELETE", "PATH_INFO" => "/articles/#{@article.id}.json" },
      StringIO.new(""),
      out,
    )
    assert_match(/^Status: 204\b/, out.string)
    # Rails omits Content-Type on 204, 205, 304, and 1xx. `head` clears
    # it for those statuses; a JSON content type here was the older shape.
    refute_includes out.string, "Content-Type:"
  end

  def test_post_articles_json_returns_201_with_location_and_body
    require_relative "../../main"
    body = "article%5Btitle%5D=Acceptance&article%5Bbody%5D=A+sufficiently+long+body."
    out = StringIO.new
    Main.run(
      {
        "REQUEST_METHOD"  => "POST",
        "PATH_INFO"       => "/articles.json",
        "CONTENT_TYPE"    => "application/x-www-form-urlencoded",
        "CONTENT_LENGTH"  => body.bytesize.to_s,
      },
      StringIO.new(body),
      out,
    )
    assert_match(/^Status: 201\b/, out.string)
    assert_includes out.string, "Content-Type: application/json"
    assert_match(%r{^Location: /articles/\d+}, out.string)
    assert_includes out.string, %("title":"Acceptance")
  end
end
