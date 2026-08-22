# Unit tests for the `Dom` selector stub in test_helper.rb — the
# substrate `assert_select` lowers to.
#
# Minitest-shaped: a CRuby-only framework test, quarantined out of the
# spin shape by `project.rs::spin_shape` (a `Minitest::Test` subclass is
# CRuby-only regardless of what else the file holds). It loads its own
# runner for the same reason route_helpers_test.rb beside it does.
#
# WHY THIS FILE EXISTS. The engine is a substring stub, so its rules are
# string rules — and a string rule that is silently wrong makes an
# assertion UNPASSABLE rather than loose, which reads as a missing
# feature in the app under test. The compound-selector rule was exactly
# that: `assert_select "turbo-frame#account_users hr.separator.full-width"`
# asked the document to contain the literal text
# `<turbo-frame#account_users`, which no emitter writes.
require "minitest/autorun"
require_relative "test_helper"

class DomTest < Minitest::Test
  HTML = <<~HTML
    <div>
      <hr class="margin-block separator full-width" style="--border-style: solid">
      <turbo-frame id="account_users">
        <hr class="separator" aria-hidden="true">
        <hr class="separator full-width" style="--border-style: solid">
      </turbo-frame>
      <a class="btn" title="Copy link" data-clipboard="http://x/1">Copy</a>
      <li id="message_4" class="message"><div class="message__body">hi</div></li>
      <template></template>
    </div>
  HTML

  def count(selector)
    Dom.select(HTML, selector).length
  end

  # The historical single-part rules, unchanged — every one of them is a
  # live campfire assertion.
  def test_tag_id_and_class_selectors
    assert_equal 1, count("template")
    assert_equal 1, count("#account_users")
    assert_equal 1, count("#message_4")
    assert_equal 1, count(".message__body")
    assert_equal 0, count("h1")
  end

  # A DESCENDANT selector targets its LAST chunk. The stub cannot scope,
  # so the ancestor is ignored; checking the ancestor INSTEAD (the old
  # rule) said nothing about the element the assertion names.
  def test_descendant_selector_targets_the_last_chunk
    assert_equal 2, count("turbo-frame#account_users hr.separator.full-width")
    assert_equal 2, count("hr.separator.full-width")
  end

  # A combinator is a chunk of its own under a whitespace split.
  def test_child_combinator_targets_the_element_after_it
    assert_equal 1, count("div > template")
  end

  # `tag#id` / `tag.cls`: the tag anchors the scan and the rest become
  # predicates on the SAME start tag.
  def test_compound_chunk_checks_every_part
    assert_equal 1, count("li#message_4")
    assert_equal 1, count("li.message")
    assert_equal 0, count("li#nope")
    assert_equal 0, count("li.nope")
  end

  # An attribute predicate may hold a SPACE, so the chunk split has to
  # be bracket-aware — a plain `split(" ")` cuts `[title='Copy link']`
  # in half.
  def test_attribute_predicate_with_a_space
    assert_equal 1, count(".btn[title='Copy link']")
    assert_equal 1, count(".btn[title='Copy link'][data-clipboard='http://x/1']")
    assert_equal 0, count(".btn[title='Copy note']")
  end

  # A predicate has to hold on the start tag the fragment matched, not
  # merely somewhere in the document.
  def test_predicate_is_scoped_to_the_matched_start_tag
    assert_equal 0, count("template[title='Copy link']")
  end

  # real-blog's `assert_select "#comments .p-4"` against
  # `class="p-4 bg-gray-50 rounded"`. The class is FIRST in the
  # attribute, which the historical "attribute ends with the name" rule
  # could not see — and the toolchain lane is what said so, after the
  # descendant-target change made this the chunk being matched.
  def test_a_class_that_is_not_last_in_the_attribute
    html = %(<div id="comments"><div class="p-4 bg-gray-50 rounded">x</div></div>)
    assert_equal 1, Dom.select(html, "#comments .p-4").length
    assert_equal 1, Dom.select(html, ".rounded").length
  end

  # WHOLE class tokens: `.message` does not hold on `message__body`.
  def test_class_matching_is_by_token_not_substring
    html = %(<div class="message__body">x</div>)
    assert_equal 0, Dom.select(html, ".message").length
    assert_equal 1, Dom.select(html, ".message__body").length
  end

  # `<hr` is a prefix of `<href-ish` too, so the tag check needs a
  # boundary.
  def test_tag_matching_stops_at_a_boundary
    assert_equal 0, Dom.select(%(<templater></templater>), "template").length
  end
end
