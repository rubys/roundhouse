require_relative "../test_helper"
require "action_view"
require "models/article"

class ViewHelpersTest < Minitest::Test
  def setup
    SchemaSetup.reset!
    ViewHelpers.reset_slots!
  end

  # ── escaping / formatting ──────────────────────────────────────

  def test_html_escape_handles_nil
    assert_equal "", ViewHelpers.html_escape(nil)
  end

  def test_html_escape_replaces_special_chars
    assert_equal "&lt;b&gt;hi&lt;/b&gt;", ViewHelpers.html_escape("<b>hi</b>")
  end

  def test_truncate_short_string_unchanged
    assert_equal "hi", ViewHelpers.truncate("hi", length: 100)
  end

  def test_truncate_long_string_with_omission
    assert_equal "abcdefg...", ViewHelpers.truncate("abcdefghijklmnop", length: 10)
  end

  def test_truncate_handles_nil
    assert_equal "", ViewHelpers.truncate(nil, length: 10)
  end

  # ── slot store ─────────────────────────────────────────────────

  def test_content_for_set_and_get
    ViewHelpers.content_for_set(:title, "Hello")
    assert_equal "Hello", ViewHelpers.content_for_get(:title)
  end

  def test_content_for_get_missing_returns_nil
    assert_nil ViewHelpers.content_for_get(:nope)
  end

  def test_get_slot_returns_empty_string_when_unset
    assert_equal "", ViewHelpers.get_slot(:head)
  end

  # ── DOM helpers ────────────────────────────────────────────────

  def test_dom_id_with_record
    article = Article.new(title: "x", body: "long body content here.")
    article.save
    assert_equal "article_#{article.id}", ViewHelpers.dom_id(article)
  end

  def test_dom_id_with_record_and_suffix
    article = Article.new(title: "x", body: "long body content here.")
    article.save
    assert_equal "article_#{article.id}_comments_count", ViewHelpers.dom_id(article, :comments_count)
  end

  def test_dom_id_with_explicit_prefix_and_id
    assert_equal "article_7", ViewHelpers.dom_id("article", 7)
  end

  # ── HTML elements ──────────────────────────────────────────────

  def test_link_to_basic
    out = ViewHelpers.link_to("Show", "/articles/42")
    assert_equal %(<a href="/articles/42">Show</a>), out
  end

  def test_link_to_with_class
    out = ViewHelpers.link_to("Show", "/articles/42", class: "btn")
    assert_includes out, %(href="/articles/42")
    assert_includes out, %(class="btn")
    assert_includes out, ">Show</a>"
  end

  def test_link_to_escapes_text
    out = ViewHelpers.link_to("<b>hi</b>", "/x")
    assert_includes out, "&lt;b&gt;hi&lt;/b&gt;"
  end

  def test_button_to_emits_form_with_method_input
    out = ViewHelpers.button_to("Delete", "/articles/42", method: :delete)
    assert_includes out, %(action="/articles/42")
    assert_includes out, %(<input type="hidden" name="_method" value="delete">)
    assert_includes out, %(<button type="submit")
    assert_includes out, ">Delete</button>"
  end

  def test_button_to_post_method_omits_hidden_input
    out = ViewHelpers.button_to("Submit", "/articles", method: :post)
    refute_includes out, "_method"
  end

  # ── stylesheet / importmap stubs ───────────────────────────────

  def test_stylesheet_link_tag
    out = ViewHelpers.stylesheet_link_tag("app")
    assert_includes out, %(rel="stylesheet")
    assert_includes out, %(href="/assets/app.css")
  end

  def test_javascript_importmap_tags_has_importmap_script
    # Helper emits the importmap script with `data-turbo-track="reload"`
    # (matching Rails' shape); assert on the opening fragment without
    # the closing `>` so the attribute can be present.
    assert_includes ViewHelpers.javascript_importmap_tags, %(<script type="importmap")
  end

  def test_javascript_importmap_tags_pins_turbo
    out = ViewHelpers.javascript_importmap_tags
    assert_includes out, %("@hotwired/turbo": "/assets/turbo.min.js")
  end

  def test_javascript_importmap_tags_imports_turbo_module
    out = ViewHelpers.javascript_importmap_tags
    assert_includes out, %(<script type="module">import "@hotwired/turbo")
  end

  def test_csrf_and_csp_meta_tags
    assert_includes ViewHelpers.csrf_meta_tags, %(name="csrf-token")
    # `csp_meta_tag` returns empty when no CSP nonce is configured —
    # matches Rails' dev-mode behavior and the other targets' runtimes.
    assert_equal "", ViewHelpers.csp_meta_tag
  end

  def test_turbo_stream_from
    out = ViewHelpers.turbo_stream_from("articles")
    assert_includes out, %(<turbo-cable-stream-source)
    assert_includes out, %(signed-stream-name="articles")
  end

  # ── form builder ───────────────────────────────────────────────

  def test_form_with_yields_builder_and_wraps_form
    article = Article.new(title: "Hello", body: "long body content here.")
    out = ViewHelpers.form_with(
      model: article,
      model_name: "article",
      action: "/articles",
      method: :post,
    ) { |f| f.text_field(:title) }
    assert_includes out, %(action="/articles")
    assert_includes out, %(method="post")
    assert_includes out, %(name="article[title]")
    assert_includes out, %(value="Hello")
  end

  def test_form_builder_label
    article = Article.new(title: "x", body: "y")
    builder = ViewHelpers::FormBuilder.new(article, "article", "/articles", :post)
    out = builder.label(:title)
    assert_includes out, %(for="article_title")
    assert_includes out, ">Title</label>"
  end

  def test_form_builder_text_field_uses_model_value
    article = Article.new(title: "Hello", body: "y")
    builder = ViewHelpers::FormBuilder.new(article, "article", "/articles", :post)
    out = builder.text_field(:title)
    assert_includes out, %(value="Hello")
  end

  def test_form_builder_text_field_handles_nil_value
    article = Article.new
    builder = ViewHelpers::FormBuilder.new(article, "article", "/articles", :post)
    out = builder.text_field(:title)
    assert_includes out, %(value="")
  end

  def test_form_with_patch_method_emits_method_override
    article = Article.new(title: "x", body: "long enough body.")
    article.save
    out = ViewHelpers.form_with(
      model: article,
      model_name: "article",
      action: "/articles/#{article.id}",
      method: :patch,
    ) { |_f| "" }
    assert_includes out, %(method="post")
    assert_includes out, %(<input type="hidden" name="_method" value="patch">)
  end
end
