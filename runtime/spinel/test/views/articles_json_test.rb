require_relative "../test_helper"
require "models/article"
require "models/comment"
require "views"

# Phase 4 acceptance for the Jbuilder lowerer pipeline. Exercises the
# three `Views::Articles.<x>_json` methods the lowerer emits from
# `fixtures/real-blog/app/views/articles/*.json.jbuilder`. Asserts on
# substring matches over the lowered JSON output rather than parsing
# (the spinel-subset `runtime/json.rb` shim only emits `JSON.generate`;
# no `parse` companion). Rails-vs-CRuby `Time#to_s` divergence and
# host-aware URL generation (`article_url` → "http://host/articles/
# 1.json") are tracked as Phase-8 follow-on concerns, not Phase-4
# blockers.

class ViewsArticlesJsonTest < Minitest::Test
  def setup
    SchemaSetup.reset!
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
    # created_at / updated_at present (exact byte format pending
    # iso8601 rewrite — Phase 8 byte-equivalence work).
    assert_includes j, %("created_at":)
    assert_includes j, %("updated_at":)
  end

  def test_article_json_partial_url_uses_path_helper
    j = Views::Articles.article_json(@article)
    # `json.url article_url(article, format: :json)` lowers through
    # the route-helper rewrite to `RouteHelpers.article_path(id)`.
    # Host + format suffix are Phase 8 work; the path component must
    # match the canonical route shape regardless.
    assert_includes j, %("url":"/articles/#{@article.id}")
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
end
