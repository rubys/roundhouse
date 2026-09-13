# Minitest-shaped, like route_helpers_test.rb: a CRuby-only framework
# test of the ruby family's `Hash#to_query` reopen
# (runtime/hash_to_query.rb over the shared runtime/action_view). The
# expectations are activesupport 8.1's own output, and they exercise
# both halves: the shared per-level sort and join, and the reopened
# nested rendering — a Hash as `outer[inner]`, an Array as `key[]` kept
# in its own order, nil as the bare key, an empty container skipped.
require "minitest/autorun"
require_relative "test_helper"
require_relative "../runtime/action_view"
require_relative "../runtime/hash_to_query"

class HashToQueryTest < Minitest::Test
  def test_nested_hash_and_array_render_as_brackets
    assert_equal "room%5Bname%5D=Designers&user_ids%5B%5D=1&user_ids%5B%5D=2",
      ActionView::ViewHelpers.to_query({ room: { name: "Designers" }, user_ids: [1, 2] })
  end

  def test_pairs_sort_per_level_and_an_array_keeps_its_order
    assert_equal "a=3&b%5Bx%5D=2&b%5By%5D=1", ActionView::ViewHelpers.to_query({ b: { y: 1, x: 2 }, a: 3 })
    assert_equal "a%5B%5D=2&a%5B%5D=1", ActionView::ViewHelpers.to_query({ a: [2, 1] })
    assert_equal "a%5Bz%5D%5B%5D=2&a%5Bz%5D%5B%5D=1", ActionView::ViewHelpers.to_query({ a: { z: [2, 1] } })
  end

  def test_nil_is_the_bare_key_and_empty_containers_vanish
    assert_equal "a&z=1", ActionView::ViewHelpers.to_query({ z: 1, a: nil, e: {}, f: [] })
  end

  def test_string_keys_and_values_are_escaped_like_cgi
    assert_equal "push_subscription%5Bauth_key%5D=a+b&push_subscription%5Bendpoint%5D=https%3A%2F%2Fx%2Fy%3Fz%3D1",
      ActionView::ViewHelpers.to_query({ push_subscription: { "endpoint" => "https://x/y?z=1", "auth_key" => "a b" } })
  end
end
