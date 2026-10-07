require_relative "../test_helper"

class IntegerKeyCastTest < Minitest::Test
  # Include underscore separators and signed zero, not only plain digits.
  def test_decimal_prefix_casting
    {
      "0" => 0, "1" => 1, "12abc" => 12, "0x10" => 0,
      "1_0" => 10, "1__0" => 1, "0_0" => 0,
      "+0001junk" => 1, "-0000junk" => 0,
      " \t\r\n\v\f-2suffix" => -2, 0 => 0, -1 => -1
    }.each do |input, expected|
      result = ActiveRecord::IntegerKeyCast.parse(input)
      assert result.valid, input.inspect
      assert_equal expected, result.value, input.inspect
    end
  end

  # Invalid text must never accidentally select a real record with ID zero.
  def test_non_numeric_inputs_are_invalid_without_becoming_zero
    [nil, "", " ", "abc", "+", "-", "_1", "  +x", "é1"].each do |input|
      refute ActiveRecord::IntegerKeyCast.parse(input).valid, input.inspect
    end
  end

  # Exercise both valid extremes and wider Ruby integers before native conversion.
  def test_signed_ranges_are_checked_before_integer_conversion
    maximum = 2**63 - 1
    minimum = -(2**63)
    [maximum, minimum].each do |expected|
      result = ActiveRecord::IntegerKeyCast.parse(expected.to_s)
      assert result.valid, expected.to_s
      assert_equal expected, result.value
    end
    [maximum + 1, minimum - 1, 10**80, -(10**80)].each do |input|
      refute ActiveRecord::IntegerKeyCast.parse(input.to_s).valid, input.to_s
      refute ActiveRecord::IntegerKeyCast.parse(input).valid, input.to_s
    end
  end

  # Float text can use an exponent; integer keys truncate the numeric value.
  def test_float_values_are_truncated_before_range_checked_lookup
    {1e-7 => 0, -1e-7 => 0, 1.9 => 1, -1.9 => -1,
     9223372036854775808.0.prev_float => 9223372036854774784,
     -9223372036854775808.0 => -9223372036854775808}.each do |input, expected|
      result = ActiveRecord::IntegerKeyCast.parse(input)
      assert result.valid, input.inspect
      assert_equal expected, result.value, input.inspect
    end
    [1e20, -1e20, 9223372036854775808.0,
     (-9223372036854775808.0).prev_float,
     Float::INFINITY, -Float::INFINITY, Float::NAN].each do |input|
      refute ActiveRecord::IntegerKeyCast.parse(input).valid, input.inspect
    end
    assert_equal 1, ActiveRecord::IntegerKeyCast.parse("1e-7").value
  end

  class Probe < ActiveRecord::Base
    # Detect invalid test inputs reaching the adapter, including a spurious row-zero lookup.
    def self._adapter_find_by_id(id)
      raise "adapter received non-integer" unless id.is_a?(Integer)
      raise "invalid input queried row zero" if id == 0
      id
    end
  end

  # The generic fallback must validate before the adapter observes a key.
  def test_find_casts_before_entering_the_typed_adapter
    assert_equal 1, Probe.find("1suffix")
    assert_equal 10, Probe.find("1_0")
    assert_equal -(2**63), Probe.find("-9223372036854775808")
    assert_equal -(2**63), Probe.find(-(2**63))
    [nil, "abc", "", " ", "9223372036854775808", "-9223372036854775809"].each do |input|
      assert_raises(ActiveRecord::RecordNotFound) { Probe.find(input) }
    end
  end

  class StringProbe < ActiveRecord::Base
    # Select the existing String-key path without adding numeric casting.
    def self._string_primary_key = true
    # Return the observed key so assertions check its exact preserved identity.
    def self._adapter_find_by_id(id) = id
  end

  # Literal String identities share no numeric-prefix normalization.
  def test_string_keys_are_preserved
    assert_equal "abc", StringProbe.find("abc")
    assert_equal "1_0", StringProbe.find("1_0")
    assert_equal "", StringProbe.find("")
    assert_equal "12", StringProbe.find(12)
    assert_raises(ActiveRecord::RecordNotFound) { StringProbe.find(nil) }
  end
end
