# Minitest-shaped, like hash_to_query_test.rb: a CRuby-only framework
# test of the ruby family's Array attribute-value reopen
# (runtime/attr_value_text.rb over the shared runtime/action_view). The
# expectations are Rails 8.1's own `link_to` output for the same calls
# — the `class:` conditional list campfire's `link_to_room` forwards
# through `**attributes`: Hash keys by truthiness, nil and "" dropped,
# no dedup, an empty list rendering `class=""`, and `href` last.
require "minitest/autorun"
require_relative "test_helper"
require_relative "../runtime/action_view"
require_relative "../runtime/attr_value_text"

class AttrValueTextTest < Minitest::Test
  def test_a_class_array_is_rails_token_list
    assert_equal %(<a class="direct" href="/r">x</a>),
      ActionView::ViewHelpers.link_to("x", "/r", class: ["direct", { unread: false }])
    assert_equal %(<a class="direct unread direct" href="/r">x</a>),
      ActionView::ViewHelpers.link_to("x", "/r", class: ["direct", { unread: true, other: nil }, "direct", nil, ""])
    assert_equal %(<a class="" href="/r">x</a>), ActionView::ViewHelpers.link_to("x", "/r", class: [])
  end

  def test_any_other_array_value_is_space_joined
    assert_equal %(<a rel="noopener noreferrer" href="/r">x</a>),
      ActionView::ViewHelpers.link_to("x", "/r", rel: ["noopener", "noreferrer"])
  end

  def test_a_scalar_is_its_to_s
    assert_equal "7", ActionView::ViewHelpers.attr_value_text("width", 7)
  end
end
