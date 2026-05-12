require_relative "test_helper"
require "json_builder"

# Direct unit tests for `runtime/ruby/json_builder.rb`. The four
# primitives the Jbuilder lowerer relies on, exercised under stock
# CRuby. Per-target transpile correctness is verified separately by
# the comparison harness against Rails reference rendering.
class JsonBuilderTest < Minitest::Test
  # ── encode_string ──────────────────────────────────────────────

  def test_encode_string_passthrough
    assert_equal "hello", JsonBuilder.encode_string("hello")
  end

  def test_encode_string_escapes_quote_and_backslash
    assert_equal "she said \\\"hi\\\"", JsonBuilder.encode_string("she said \"hi\"")
    assert_equal "a\\\\b", JsonBuilder.encode_string("a\\b")
  end

  def test_encode_string_escapes_whitespace_controls
    assert_equal "a\\nb\\tc\\rd", JsonBuilder.encode_string("a\nb\tc\rd")
  end

  def test_encode_string_nil_returns_empty
    assert_equal "", JsonBuilder.encode_string(nil)
  end

  # ── encode_value ───────────────────────────────────────────────

  def test_encode_value_nil
    assert_equal "null", JsonBuilder.encode_value(nil)
  end

  def test_encode_value_bool
    assert_equal "true", JsonBuilder.encode_value(true)
    assert_equal "false", JsonBuilder.encode_value(false)
  end

  def test_encode_value_integer
    assert_equal "0", JsonBuilder.encode_value(0)
    assert_equal "-7", JsonBuilder.encode_value(-7)
    assert_equal "42", JsonBuilder.encode_value(42)
  end

  def test_encode_value_float
    assert_equal "3.14", JsonBuilder.encode_value(3.14)
  end

  def test_encode_value_string_is_quoted
    assert_equal "\"hello\"", JsonBuilder.encode_value("hello")
  end

  def test_encode_value_string_escapes_inside_quotes
    assert_equal "\"a\\\"b\"", JsonBuilder.encode_value("a\"b")
  end

end
