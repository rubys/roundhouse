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

  # ── encode_datetime ────────────────────────────────────────────

  def test_encode_datetime_nil
    assert_equal "null", JsonBuilder.encode_datetime(nil)
  end

  def test_encode_datetime_full_microseconds
    # Sqlite TEXT timestamp with microsecond fraction.
    assert_equal "\"2026-05-10T02:22:28.114Z\"",
      JsonBuilder.encode_datetime("2026-05-10 02:22:28.114670")
  end

  def test_encode_datetime_no_fraction
    # No fractional seconds → milliseconds default to "000".
    assert_equal "\"2026-05-10T02:22:28.000Z\"",
      JsonBuilder.encode_datetime("2026-05-10 02:22:28")
  end

  def test_encode_datetime_short_fraction
    # One-digit fraction pads to milliseconds.
    assert_equal "\"2026-05-10T02:22:28.100Z\"",
      JsonBuilder.encode_datetime("2026-05-10 02:22:28.1")
  end

  def test_encode_datetime_unrecognized_passes_through_as_string
    # Bogus input → fallback quoted-string encoding so call sites
    # don't crash on malformed column data.
    assert_equal "\"oops\"", JsonBuilder.encode_datetime("oops")
  end

  # ── parse_object / read_raw / decode_string ────────────────────
  #
  # Parsed objects are inspected through `read_raw` rather than
  # `parse_object(x)[k]` or `.length`: a Hash literal in statement
  # position is a BLOCK in JavaScript, `!==` on two objects compares
  # references, and the Hash method surface is selected by the
  # receiver's declared type — which a local assigned from a call does
  # not carry in the runtime transpile. `read_raw` answers `""` for an
  # absent key, which is unambiguous (the shortest JSON value text is
  # two characters).

  def test_parse_object_empty_sources
    assert_equal "", JsonBuilder.read_raw(JsonBuilder.parse_object(nil), "a")
    assert_equal "", JsonBuilder.read_raw(JsonBuilder.parse_object(""), "a")
    assert_equal "", JsonBuilder.read_raw(JsonBuilder.parse_object("{}"), "a")
  end

  def test_parse_object_keeps_values_as_source_text
    doc = JsonBuilder.parse_object("{\"a\":true,\"b\":42,\"c\":\"hi\",\"d\":null}")
    assert_equal "true", JsonBuilder.read_raw(doc, "a")
    assert_equal "42", JsonBuilder.read_raw(doc, "b")
    assert_equal "\"hi\"", JsonBuilder.read_raw(doc, "c")
    assert_equal "null", JsonBuilder.read_raw(doc, "d")
  end

  def test_parse_object_tolerates_whitespace
    doc = JsonBuilder.parse_object("{ \"a\" : 1 , \"b\" : 2 }")
    assert_equal "1", JsonBuilder.read_raw(doc, "a")
    assert_equal "2", JsonBuilder.read_raw(doc, "b")
  end

  def test_parse_object_handles_escaped_quote_in_key_and_value
    doc = JsonBuilder.parse_object("{\"a\\\"b\":\"c\\\"d\"}")
    assert_equal "\"c\\\"d\"", JsonBuilder.read_raw(doc, "a\"b")
  end

  def test_parse_object_stops_at_a_nested_value
    # Nesting is outside what has_json permits; the scan truncates
    # rather than mis-splitting on the inner commas.
    doc = JsonBuilder.parse_object("{\"a\":{\"b\":1},\"c\":2}")
    assert_equal "", JsonBuilder.read_raw(doc, "a")
    assert_equal "", JsonBuilder.read_raw(doc, "c")
  end

  def test_render_object_round_trips_and_sorts
    assert_equal "{\"a\":\"x\",\"b\":1}",
      JsonBuilder.render_object(JsonBuilder.parse_object("{\"b\":1,\"a\":\"x\"}"))
  end

  def test_decode_string_unescapes
    assert_equal "a\"b", JsonBuilder.decode_string("\"a\\\"b\"")
    assert_equal "a\nb", JsonBuilder.decode_string("\"a\\nb\"")
    assert_equal "a\\b", JsonBuilder.decode_string("\"a\\\\b\"")
  end

  # ── read_* ─────────────────────────────────────────────────────

  def test_read_boolean
    doc = "{\"on\":true,\"off\":false,\"void\":null}"
    assert_equal true, JsonBuilder.read_boolean(doc, "on", false)
    assert_equal false, JsonBuilder.read_boolean(doc, "off", true)
    # A stored null and an absent key both answer the declared default.
    assert_equal true, JsonBuilder.read_boolean(doc, "void", true)
    assert_equal true, JsonBuilder.read_boolean(doc, "missing", true)
    assert_equal false, JsonBuilder.read_boolean(nil, "on", false)
  end

  def test_read_integer
    doc = "{\"n\":42,\"neg\":-7}"
    assert_equal 42, JsonBuilder.read_integer(doc, "n", 0)
    assert_equal(-7, JsonBuilder.read_integer(doc, "neg", 0))
    assert_equal 5, JsonBuilder.read_integer(doc, "missing", 5)
  end

  def test_read_string
    doc = "{\"greeting\":\"hello\"}"
    assert_equal "hello", JsonBuilder.read_string(doc, "greeting", "hi")
    assert_equal "hi", JsonBuilder.read_string(doc, "missing", "hi")
  end

  # ── write_* ────────────────────────────────────────────────────

  def test_write_into_an_empty_column
    assert_equal "{\"on\":true}", JsonBuilder.write_boolean(nil, "on", true)
    assert_equal "{\"n\":42}", JsonBuilder.write_integer("", "n", 42)
    assert_equal "{\"s\":\"hi\"}", JsonBuilder.write_string("{}", "s", "hi")
  end

  def test_write_preserves_untouched_keys_and_sorts
    doc = "{\"b\":1,\"a\":\"x\"}"
    assert_equal "{\"a\":\"x\",\"b\":1,\"c\":false}",
      JsonBuilder.write_boolean(doc, "c", false)
  end

  def test_write_replaces_an_existing_key
    assert_equal "{\"on\":false}", JsonBuilder.write_boolean("{\"on\":true}", "on", false)
  end

  def test_write_escapes_the_value
    assert_equal "{\"s\":\"a\\\"b\"}", JsonBuilder.write_string("{}", "s", "a\"b")
  end

  def test_write_then_read_round_trips
    doc = JsonBuilder.write_string(JsonBuilder.write_boolean(nil, "on", true), "s", "a\"b")
    assert_equal true, JsonBuilder.read_boolean(doc, "on", false)
    assert_equal "a\"b", JsonBuilder.read_string(doc, "s", "")
  end
end
