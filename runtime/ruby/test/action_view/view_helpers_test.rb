require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_view/view_helpers.rb`.
# Promoted from fixtures/spinel-blog/test/runtime/view_helpers_test.rb;
# the framework version uses lightweight stand-ins instead of
# Article (which couples to a schema + DB). Same coverage on the
# framework surface.
# Smallest record-shaped object the helpers need: `id` for
# dom_id, `class.name` for record_dom_prefix's downcase, `[]`
# for FormBuilder field lookups (model[:title]). Subclasses
# ActiveRecord::Base so it satisfies FormBuilder's typed
# constructor under strict-typed targets (Crystal types
# `model : ActiveRecord::Base`); `Struct.new` would yield a
# standalone Struct that doesn't fit that slot. Defined at top
# level (not nested under ViewHelpersTest) so `Article.name`
# returns the bare "Article" in Ruby — `dom_id` then renders
# `"article_<id>"` to match Rails. (TS/Crystal already see the
# emit as a top-level class regardless of where the source
# nested it.)
class Article < ActiveRecord::Base
  attr_accessor :title, :body

  # String defaults (not nil) for `title`/`body` so Crystal's
  # strict-typing infers @title/@body as `String`. The nil-handling
  # text_field test below uses an explicit `""` value — equivalent
  # to nil for the helper's value-omission contract (`text_field`
  # omits the `value` attribute for nil OR empty).
  def initialize(id = 0, title = "", body = "")
    super()
    self.id = id
    @title = title
    @body = body
  end

  def [](field)
    case field
    when :id then @id
    when :title then @title
    when :body then @body
    end
  end

  # Per-model `dom_prefix` — the lowerer synthesizes this for
  # app/models/ classes via `push_dom_prefix_method`; test stubs
  # defined inline here have to declare it explicitly so dom_id's
  # contract `record.dom_prefix` resolves.
  def dom_prefix
    "article"
  end
end

class ViewHelpersTest < Minitest::Test
  # Bring `ActionView::ViewHelpers` into scope as `ViewHelpers`
  # for test readability AND `ActionView::ViewHelpers::FormBuilder`
  # into scope as bare `FormBuilder`. The TS emit treats the latter
  # as a top-level export (each framework class collapses to its
  # leaf name, sibling exports), so the bare-name reference matches
  # what the transpile produces.
  include ActionView
  include ActionView::ViewHelpers

  def setup
    ViewHelpers.reset_slots!
  end

  # ── escaping / formatting ──────────────────────────────────

  # html_escape was previously polymorphic (accepted nil and stringified
  # internally). The monomorphic contract — `(String) -> String` —
  # pushes that responsibility to callers. `render_attrs` and
  # `text_area` (the only internal callers that previously passed
  # non-String) now wrap values in `.to_s` at the call site. The
  # nil-passing behavior is intentionally no longer supported.

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

  # truncate is monomorphic — `(String, length:, omission:) -> String`.
  # The previous nil-handling came with html_escape's nil-handling;
  # both are now caller responsibility.

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

  # `dom_id("article", 7)` (explicit String prefix + integer suffix)
  # is no longer supported — `dom_id` is monomorphic on a record
  # receiver. Callers needing the explicit form spell it directly
  # in source: `"article_7"`.

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
    # `.to_h` on each pin: Ruby no-op, Crystal converts the NamedTuple
    # literal to Hash so the array element type matches the helper's
    # `Array(Hash(Symbol, String))` parameter signature.
    pins = [{ name: "app", path: "/assets/app.js" }.to_h]
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

  def test_csrf_token_hidden_input
    out = ViewHelpers.csrf_token_hidden_input
    assert_equal %(<input type="hidden" name="authenticity_token" value="">), out
  end

  def test_method_override_input_emits_for_patch
    out = ViewHelpers.method_override_input(:patch)
    assert_equal %(<input type="hidden" name="_method" value="patch">), out
  end

  def test_method_override_input_emits_for_delete
    assert_equal %(<input type="hidden" name="_method" value="delete">),
                 ViewHelpers.method_override_input(:delete)
  end

  def test_method_override_input_empty_for_get_post
    assert_equal "", ViewHelpers.method_override_input(:get)
    assert_equal "", ViewHelpers.method_override_input(:post)
  end

  def test_optional_value_attr_emits_for_non_empty
    assert_equal %( value="hello"), ViewHelpers.optional_value_attr("hello")
  end

  def test_optional_value_attr_escapes_value
    assert_equal %( value="&lt;tag&gt;"), ViewHelpers.optional_value_attr("<tag>")
  end

  def test_optional_value_attr_empty_for_nil_or_blank
    assert_equal "", ViewHelpers.optional_value_attr(nil)
    assert_equal "", ViewHelpers.optional_value_attr("")
  end

  def test_escape_or_empty_returns_escaped_value
    assert_equal "hello", ViewHelpers.escape_or_empty("hello")
    assert_equal "&lt;b&gt;hi&lt;/b&gt;", ViewHelpers.escape_or_empty("<b>hi</b>")
  end

  def test_escape_or_empty_returns_empty_for_nil
    assert_equal "", ViewHelpers.escape_or_empty(nil)
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

  # FormBuilder + form_with tests retired alongside the runtime
  # classes themselves — the lowerer macro-inlines form_with and
  # form.label/text_field/text_area/submit at lower time (Stages 1a
  # + 1b-i + 1b-ii). Equivalent compare-gate coverage is now
  # exercised end-to-end via fixtures/real-blog's /articles/new and
  # /articles/1/edit paths through the compare-ruby + compare-ts +
  # compare-rust gates.
end
