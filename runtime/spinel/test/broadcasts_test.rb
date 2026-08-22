# Minitest-shaped: this is a CRuby-only framework test, quarantined
# out of the spin shape by `project.rs::spin_shape` (it is not a spin
# test program and gets no snapshot). It therefore loads its own
# runner — `test_helper` deliberately does not, because the EMITTED
# tests inherit `TestBase` and carry their own driver shim.
require "minitest/autorun"
# `test_helper` already loads `Broadcasts` — through
# `runtime/broadcasts` here and through `boot.rb` in an emitted tree,
# where this file also ships. Naming it again as `require "broadcasts"`
# resolved only on a load path the emit does not have, which is why the
# emitted copy could not run and this file sat ungated.
require_relative "test_helper"

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

  # ── the two capture helpers really do differ ─────────────────────
  #
  # `assert_turbo_stream_broadcasts` is CUMULATIVE at the turbo-rails
  # version campfire pins (2.0.16/2.0.17/2.0.20 run the block and then
  # read the whole stream; the `new_broadcasts_from` delta arrives in
  # 2.0.21). `assert_broadcasts` is Action Cable's own and takes a delta
  # in every version. Two blocks with one broadcast each therefore
  # answer 1 then 2 for the first, and 1 then 1 for the second — which
  # is exactly what campfire's `rooms/involvements_controller_test`
  # asserts, and what a delta on both got wrong.

  class CaptureHarness < TestBase
    def response = nil
  end

  def test_turbo_stream_capture_is_cumulative_across_blocks
    h = CaptureHarness.new
    Broadcasts.reset_log!
    first = h.capture_turbo_stream_broadcasts("designers") do
      Broadcasts.append(stream: "designers", target: "t", html: "one")
    end
    second = h.capture_turbo_stream_broadcasts("designers") do
      Broadcasts.append(stream: "designers", target: "t", html: "two")
    end
    assert_equal 1, first.length
    assert_equal 2, second.length, "the second block must see the first block's broadcast too"
  end

  # Same producer, same stream, same two blocks — so the only thing
  # this can be measuring is the helper.
  def test_action_cable_capture_stays_a_delta
    h = CaptureHarness.new
    Broadcasts.reset_log!
    first = h.capture_broadcasts_on("user_1_unreads") do
      Broadcasts.append(stream: "user_1_unreads", target: "t", html: "one")
    end
    second = h.capture_broadcasts_on("user_1_unreads") do
      Broadcasts.append(stream: "user_1_unreads", target: "t", html: "two")
    end
    assert_equal 1, first.length
    assert_equal 1, second.length, "assert_broadcasts counts only what its own block added"
  end
end
