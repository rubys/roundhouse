# frozen_string_literal: true
require_relative "../test_helper"

# `ActionDispatch::RequestId.make` and `ActionDispatch::Runtime`
# (action_dispatch/request_id.rb). Each expected id is what a Rails
# 8.1.4 app's response carried in `x-request-id` for that request
# header, through the full default middleware stack.
class RequestIdTest < Minitest::Test
  UUID = /\A\h{8}-\h{4}-\h{4}-\h{4}-\h{12}\z/

  def test_an_absent_or_blank_header_gets_a_fresh_uuid
    a = ActionDispatch::RequestId.make("")
    b = ActionDispatch::RequestId.make("   ")
    assert_match UUID, a
    assert_match UUID, b
    refute_equal a, b
  end

  def test_the_client_id_is_kept_without_other_characters
    assert_equal "abc-DEF_1@xyz", ActionDispatch::RequestId.make("abc-DEF_1@x!y z")
    assert_equal "abcd", ActionDispatch::RequestId.make("ab\tcd")
    assert_equal "w-ab", ActionDispatch::RequestId.make("żółw-ab".b)
    assert_equal "w-ab", ActionDispatch::RequestId.make("żółw-ab")
  end

  # Presence is checked before the cleanup.
  def test_an_id_of_other_characters_answers_empty
    assert_equal "", ActionDispatch::RequestId.make("!!!")
  end

  def test_the_id_is_cut_to_255
    assert_equal "a" * 255, ActionDispatch::RequestId.make("a" * 300)
    assert_equal "a" * 255, ActionDispatch::RequestId.make("!" + "a" * 300)
  end

  def test_runtime_is_seconds_with_six_decimals
    started = ActionDispatch::Runtime.clock_time - 1.25
    value = ActionDispatch::Runtime.header_value(started)
    assert_match(/\A1\.25\d{4}\z/, value)
  end
end
