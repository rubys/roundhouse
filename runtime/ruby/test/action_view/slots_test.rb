require_relative "../test_helper"

# Direct unit tests for runtime/ruby/action_view/slots.rb.
# Promoted ahead of the lowerer change that wires Slots through the
# controller→view chain — locks the value-object surface before the
# cross-target refactor depends on it.
class SlotsTest < Minitest::Test
  include ActionView

  def test_get_returns_nil_when_unset
    slots = Slots.new
    assert_nil slots.get(:title)
  end

  def test_set_then_get_roundtrips
    slots = Slots.new
    slots.set(:title, "Hello")
    assert_equal "Hello", slots.get(:title)
  end

  def test_set_returns_nil_not_value
    slots = Slots.new
    assert_nil slots.set(:title, "Hello")
  end

  def test_bracket_get_returns_empty_string_when_unset
    slots = Slots.new
    assert_equal "", slots.bracket_get(:head)
  end

  def test_bracket_get_returns_stored_value
    slots = Slots.new
    slots.set(:head, "<title>x</title>")
    assert_equal "<title>x</title>", slots.bracket_get(:head)
  end

  def test_get_yield_returns_empty_when_unset
    slots = Slots.new
    assert_equal "", slots.get_yield
  end

  def test_set_yield_then_get_yield_roundtrip
    slots = Slots.new
    slots.set_yield("<body>")
    assert_equal "<body>", slots.get_yield
  end

  def test_reset_clears_all_slots
    slots = Slots.new
    slots.set_yield("body")
    slots.set(:head, "<title>x</title>")
    slots.reset!
    assert_equal "", slots.get_yield
    assert_nil slots.get(:head)
  end

  def test_distinct_instances_do_not_share_state
    a = Slots.new
    b = Slots.new
    a.set(:title, "A")
    b.set(:title, "B")
    assert_equal "A", a.get(:title)
    assert_equal "B", b.get(:title)
  end
end
