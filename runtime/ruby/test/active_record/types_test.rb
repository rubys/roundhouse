# frozen_string_literal: true
require_relative "../test_helper"

class ActiveRecordTypesTest < Minitest::Test
  def test_string_cast
    caster = ActiveRecord::Type::String.new
    assert_nil caster.cast(nil)
    assert_equal "t", caster.cast(true)
    assert_equal "f", caster.cast(false)
    assert_equal "12", caster.cast(12)
    assert_equal "12", caster.deserialize(12)
  end

  def test_integer_cast
    caster = ActiveRecord::Type::Integer.new
    assert_nil caster.cast(nil)
    assert_nil caster.cast("")
    assert_nil caster.cast("  ")
    assert_nil caster.cast(:no)
    assert_equal 1, caster.cast(true)
    assert_equal 0, caster.cast(false)
    assert_equal 12, caster.cast(12)
    assert_equal 12, caster.cast("12")
    assert_equal 12, caster.deserialize("12")
  end
end
