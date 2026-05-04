require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_view/view_helpers.rb`.
# Promoted from fixtures/spinel-blog/test/runtime/view_helpers_test.rb;
# the framework version uses lightweight stand-ins instead of
# Article (which couples to a schema + DB). Same coverage on the
# framework surface.
class ViewHelpersTest < Minitest::Test
  # Smallest record-shaped object the helpers need: `id` for
  # dom_id, `class.name` for record_dom_prefix's downcase, `[]`
  # for FormBuilder field lookups (model[:title]). Override `name`
  # on the singleton class so the nested `ViewHelpersTest::Article`
  # path collapses to `"Article"` — the dom_id contract matches
  # Rails' top-level convention regardless of where the test
  # model lives in the constant tree.
  Article = Struct.new(:id, :title, :body) do
    def [](field) = send(field)
    def self.name = "Article"
  end

  def setup
    ViewHelpers.reset_slots!
  end

  # ── escaping / formatting ──────────────────────────────────

  def test_html_escape_handles_nil
    assert_equal "", ViewHelpers.html_escape(nil)
  end

  def test_html_escape_replaces_special_chars
    assert_equal "&lt;b&gt;hi&lt;/b&gt;", ViewHelpers.html_escape("<b>hi</b>")
  end

  def test_html_escape_handles_quotes_and_apostrophes
    assert_equal "&quot;hi&quot; &amp; &#39;bye&#39;",
      ViewHelpers.html_escape(%("hi" & 'bye'))
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

  def test_truncate_custom_omission
    assert_equal "abc[…]", ViewHelpers.truncate("abcdefghij", length: 6, omission: "[…]")
  end

  # ── slot store ─────────────────────────────────────────────

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

  def test_get_yield_returns_empty_when_unset
    assert_equal "", ViewHelpers.get_yield
  end

  def test_set_yield_then_get_yield_roundtrip
    ViewHelpers.set_yield("<body>")
    assert_equal "<body>", ViewHelpers.get_yield
  end

  def test_reset_slots_clears_yield_and_content_for
    ViewHelpers.set_yield("body")
    ViewHelpers.content_for_set(:head, "<title>x</title>")
    ViewHelpers.reset_slots!
    assert_equal "", ViewHelpers.get_yield
    assert_nil ViewHelpers.content_for_get(:head)
  end

  # ── DOM helpers ────────────────────────────────────────────

  def test_dom_id_with_record
    article = Article.new(7, "Hi", "body")
    assert_equal "article_7", ViewHelpers.dom_id(article)
  end

  def test_dom_id_with_record_and_suffix
    # Rails' dom_id puts the suffix BEFORE the model_name+id.
    article = Article.new(3, "Hi", "body")
    assert_equal "comments_count_article_3",
      ViewHelpers.dom_id(article, :comments_count)
  end

  def test_dom_id_with_explicit_prefix_and_id
    assert_equal "article_7", ViewHelpers.dom_id("article", 7)
  end

  # ── HTML elements ──────────────────────────────────────────

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
    # Default form class is `button_to` (Rails' convention) when the
    # caller doesn't pass `form_class:`. CSRF authenticity_token input
    # also lands inside the form (after the button).
    assert_includes out, %(class="button_to")
    assert_includes out, %(<input type="hidden" name="authenticity_token" value="">)
  end

  def test_button_to_post_method_omits_hidden_input
    out = ViewHelpers.button_to("Submit", "/articles", method: :post)
    refute_includes out, "_method"
  end

  def test_button_to_custom_form_class
    out = ViewHelpers.button_to("X", "/x", method: :delete, form_class: "inline-form")
    assert_includes out, %(class="inline-form")
    refute_includes out, %(class="button_to")
  end

  # ── stylesheet / importmap stubs ───────────────────────────

  def test_stylesheet_link_tag
    out = ViewHelpers.stylesheet_link_tag("app")
    assert_includes out, %(rel="stylesheet")
    assert_includes out, %(href="/assets/app.css")
  end

  def test_javascript_importmap_tags_has_importmap_script
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

  def test_javascript_importmap_tags_with_explicit_pins
    pins = [{ name: "app", path: "/assets/app.js" }]
    out = ViewHelpers.javascript_importmap_tags(pins, "app")
    assert_includes out, %("app": "/assets/app.js")
    assert_includes out, %(<link rel="modulepreload" href="/assets/app.js">)
    assert_includes out, %(<script type="module">import "app")
  end

  def test_csrf_meta_tags
    out = ViewHelpers.csrf_meta_tags
    assert_includes out, %(name="csrf-token")
    assert_includes out, %(name="csrf-param")
  end

  def test_csp_meta_tag_returns_empty_when_unconfigured
    # Matches Rails' dev-mode behavior and the other targets' runtimes.
    assert_equal "", ViewHelpers.csp_meta_tag
  end

  def test_turbo_stream_from
    out = ViewHelpers.turbo_stream_from("articles")
    assert_includes out, %(<turbo-cable-stream-source)
    # signed-stream-name carries base64(JSON("articles")) +
    # `--unsigned` suffix; the compare harness strips the HMAC
    # suffix so this aligns with Rails' signed value.
    assert_includes out, %(signed-stream-name="ImFydGljbGVzIg==--unsigned")
  end

  # ── form builder ───────────────────────────────────────────

  def test_form_with_yields_builder_and_wraps_form
    article = Article.new(0, "Hello", "")
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
    article = Article.new(0, "x", "y")
    builder = ViewHelpers::FormBuilder.new(article, "article", "/articles", :post)
    out = builder.label(:title)
    assert_includes out, %(for="article_title")
    assert_includes out, ">Title</label>"
  end

  def test_form_builder_text_field_uses_model_value
    article = Article.new(0, "Hello", "y")
    builder = ViewHelpers::FormBuilder.new(article, "article", "/articles", :post)
    out = builder.text_field(:title)
    assert_includes out, %(value="Hello")
  end

  def test_form_builder_text_field_handles_nil_value
    article = Article.new(0, nil, nil)
    builder = ViewHelpers::FormBuilder.new(article, "article", "/articles", :post)
    out = builder.text_field(:title)
    # Rails omits the `value` attribute when the field is nil/empty
    # rather than emitting `value=""`; spinel matches.
    refute_includes out, %(value=)
    assert_includes out, %(name="article[title]")
  end

  def test_form_builder_text_area
    article = Article.new(0, "x", "Hello body")
    builder = ViewHelpers::FormBuilder.new(article, "article", "/articles", :post)
    out = builder.text_area(:body)
    assert_includes out, %(name="article[body]")
    assert_includes out, ">Hello body</textarea>"
  end

  def test_form_builder_submit_default_label_for_post
    article = Article.new(0, "x", "y")
    builder = ViewHelpers::FormBuilder.new(article, "article", "/articles", :post)
    out = builder.submit
    assert_includes out, %(value="Create Article")
    assert_includes out, %(data-disable-with="Create Article")
  end

  def test_form_builder_submit_default_label_for_patch
    article = Article.new(1, "x", "y")
    builder = ViewHelpers::FormBuilder.new(article, "article", "/articles/1", :patch)
    out = builder.submit
    assert_includes out, %(value="Update Article")
  end

  def test_form_with_patch_method_emits_method_override
    article = Article.new(1, "x", "long enough body.")
    out = ViewHelpers.form_with(
      model: article,
      model_name: "article",
      action: "/articles/1",
      method: :patch,
    ) { |_f| "" }
    # Rails: `<form method="post">` (browsers don't support PATCH);
    # the `_method` hidden input carries the real verb.
    assert_includes out, %(method="post")
    assert_includes out, %(<input type="hidden" name="_method" value="patch">)
    assert_includes out, %(<input type="hidden" name="authenticity_token" value="">)
    assert_includes out, %(accept-charset="UTF-8")
  end
end
