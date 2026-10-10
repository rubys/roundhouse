require "minitest/autorun"
require_relative "../hash_deep_merge"

class HashDeepMergeTest < Minitest::Test
  def test_recursively_merges_hashes_and_replaces_every_other_value
    left = {
      nested: { keep: "left", conflict: "left", subtree: { old: true } },
      array: [1, 2],
      hash_to_scalar: { old: true },
      scalar_to_hash: "old",
      left_only: :left
    }
    right = {
      nested: { conflict: "right", added: "right", subtree: { new: true } },
      array: [3],
      hash_to_scalar: "right",
      scalar_to_hash: { added: true },
      right_only: :right
    }

    assert_equal({
      nested: { keep: "left", conflict: "right", added: "right", subtree: { old: true, new: true } },
      array: [3],
      hash_to_scalar: "right",
      scalar_to_hash: { added: true },
      left_only: :left,
      right_only: :right
    }, ActiveSupport.deep_merge(left, right))
    assert_equal({
      nested: { keep: "left", conflict: "left", subtree: { old: true } },
      array: [1, 2],
      hash_to_scalar: { old: true },
      scalar_to_hash: "old",
      left_only: :left
    }, left)
    assert_equal({
      nested: { conflict: "right", added: "right", subtree: { new: true } },
      array: [3],
      hash_to_scalar: "right",
      scalar_to_hash: { added: true },
      right_only: :right
    }, right)
    refute_same left, ActiveSupport.deep_merge(left, {})
    assert_equal left, ActiveSupport.deep_merge(left, {})
    assert_equal right, ActiveSupport.deep_merge({}, right)
    assert_equal({}, ActiveSupport.deep_merge({}, {}))
  end

  def test_absent_keys_do_not_merge_hash_defaults
    left = Hash.new({ nested: { default: true } })
    right = { missing: { nested: { right: true } } }

    assert_equal right, ActiveSupport.deep_merge(left, right)
  end
end
