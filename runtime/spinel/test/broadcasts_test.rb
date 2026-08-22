# Minitest-shaped: this is a CRuby-only framework test, quarantined
# out of the spin shape by `project.rs::spin_shape` (it is not a spin
# test program and gets no snapshot). It therefore loads its own
# runner — `test_helper` deliberately does not, because the EMITTED
# tests inherit `TestBase` and carry their own driver shim.
require "minitest/autorun"
require_relative "test_helper"
require "broadcasts"

class BroadcastsTest < Minitest::Test
  def setup
    Broadcasts.reset_log!
  end

  def test_append_records_action
    Broadcasts.append(stream: "articles", target: "articles", html: "<p>hi</p>")
    entry = Broadcasts.log.first
    assert_equal :append, entry[:action]
    assert_equal "articles", entry[:stream]
    assert_equal "articles", entry[:target]
    assert_equal "<p>hi</p>", entry[:html]
  end

  def test_prepend_records_action
    Broadcasts.prepend(stream: "s", target: "t", html: "<p>")
    assert_equal :prepend, Broadcasts.log.first[:action]
  end

  def test_replace_records_action
    Broadcasts.replace(stream: "s", target: "t", html: "<p>")
    assert_equal :replace, Broadcasts.log.first[:action]
  end

  def test_remove_records_with_empty_html
    Broadcasts.remove(stream: "s", target: "article_1")
    entry = Broadcasts.log.first
    assert_equal :remove, entry[:action]
    assert_equal "", entry[:html]
  end

  def test_log_accumulates_in_order
    Broadcasts.append(stream: "s", target: "t1", html: "a")
    Broadcasts.replace(stream: "s", target: "t2", html: "b")
    Broadcasts.remove(stream: "s", target: "t3")
    assert_equal 3, Broadcasts.log.length
    assert_equal [:append, :replace, :remove], Broadcasts.log.map { |e| e[:action] }
  end

  def test_reset_log_clears
    Broadcasts.append(stream: "s", target: "t", html: "a")
    Broadcasts.reset_log!
    assert_equal 0, Broadcasts.log.length
  end

  def test_log_returns_a_copy
    Broadcasts.append(stream: "s", target: "t", html: "a")
    snapshot = Broadcasts.log
    Broadcasts.append(stream: "s", target: "t2", html: "b")
    assert_equal 1, snapshot.length, "snapshot should not see later additions"
  end

  # ── render_fragment ──────────────────────────────────────────────

  def test_render_fragment_replace_includes_template
    out = Broadcasts.render_fragment(action: :replace, target: "article_1", html: "<p>x</p>")
    assert_includes out, %(<turbo-stream action="replace" target="article_1">)
    assert_includes out, "<template><p>x</p></template>"
    assert_includes out, "</turbo-stream>"
  end

  def test_render_fragment_remove_omits_template
    out = Broadcasts.render_fragment(action: :remove, target: "article_1")
    assert_includes out, %(<turbo-stream action="remove" target="article_1">)
    refute_includes out, "<template>"
  end

  def test_render_fragment_append_with_default_empty_html
    out = Broadcasts.render_fragment(action: :append, target: "items")
    assert_includes out, %(action="append")
    assert_includes out, "<template></template>"
  end

  # Custom element attributes ride AHEAD of action/target, which is
  # where turbo-rails' `tag.turbo_stream(template, **attributes,
  # action:, target:)` writes them. Byte-checked against ActionView 8.1
  # rather than assumed — the key keeps its underscore (nothing
  # dasherizes it) and `true` renders as the string "true".
  def test_render_fragment_writes_attributes_before_action
    out = Broadcasts.render_fragment(
      action: :append, target: "items", html: "<p>x</p>",
      attributes: %( maintain_scroll="true")
    )
    assert_equal(
      %(<turbo-stream maintain_scroll="true" action="append" target="items">) +
      "<template><p>x</p></template></turbo-stream>",
      out
    )
  end

  def test_render_fragment_remove_carries_attributes_too
    out = Broadcasts.render_fragment(
      action: :remove, target: "items", attributes: %( maintain_scroll="true")
    )
    assert_equal %(<turbo-stream maintain_scroll="true" action="remove" target="items"></turbo-stream>), out
  end

  # The three-argument spelling every view lowerer emits is unchanged,
  # so no existing turbo_stream template moves.
  def test_turbo_stream_fragment_without_attributes_is_unchanged
    assert_equal(
      %(<turbo-stream action="append" target="items"><template>hi</template></turbo-stream>),
      Broadcasts.turbo_stream_fragment("append", "items", "hi")
    )
  end
end
