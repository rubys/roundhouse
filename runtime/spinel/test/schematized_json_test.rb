# Requires the two runtime files directly rather than `test_helper`:
# this module needs nothing from the app tree (no DB, no schema, no
# fixtures), and the pair is exactly what `runtime/schematized_json.rb`
# depends on. Run by `tests/ruby_toolchain.rs`, which overlays this
# directory onto the generated tree.
require "minitest/autorun"
require_relative "../runtime/json_builder"
require_relative "../runtime/schematized_json"

# Unit tests for `runtime/schematized_json.rb` — the `has_json`
# (ActiveModel::SchematizedJson) column seam that `lower::has_json`
# routes every synthesized accessor through. Sibling of cgi_io_test:
# a per-target runtime module tested in the tree it ships to.
class SchematizedJsonTest < Minitest::Test
  # ── parse_object / read_raw / decode_string ────────────────────
  #
  # Parsed objects are inspected through `read_raw`, which answers `""`
  # for an absent key — unambiguous, since the shortest JSON value text
  # is two characters.

  def test_parse_object_empty_sources
    assert_equal "", SchematizedJson.read_raw(SchematizedJson.parse_object(nil), "a")
    assert_equal "", SchematizedJson.read_raw(SchematizedJson.parse_object(""), "a")
    assert_equal "", SchematizedJson.read_raw(SchematizedJson.parse_object("{}"), "a")
  end

  def test_parse_object_keeps_values_as_source_text
    doc = SchematizedJson.parse_object("{\"a\":true,\"b\":42,\"c\":\"hi\",\"d\":null}")
    assert_equal "true", SchematizedJson.read_raw(doc, "a")
    assert_equal "42", SchematizedJson.read_raw(doc, "b")
    assert_equal "\"hi\"", SchematizedJson.read_raw(doc, "c")
    assert_equal "null", SchematizedJson.read_raw(doc, "d")
  end

  def test_parse_object_tolerates_whitespace
    doc = SchematizedJson.parse_object("{ \"a\" : 1 , \"b\" : 2 }")
    assert_equal "1", SchematizedJson.read_raw(doc, "a")
    assert_equal "2", SchematizedJson.read_raw(doc, "b")
  end

  def test_parse_object_handles_escaped_quote_in_key_and_value
    doc = SchematizedJson.parse_object("{\"a\\\"b\":\"c\\\"d\"}")
    assert_equal "\"c\\\"d\"", SchematizedJson.read_raw(doc, "a\"b")
  end

  def test_parse_object_stops_at_a_nested_value
    # Nesting is outside what has_json permits; the scan truncates
    # rather than mis-splitting on the inner commas.
    doc = SchematizedJson.parse_object("{\"a\":{\"b\":1},\"c\":2}")
    assert_equal "", SchematizedJson.read_raw(doc, "a")
    assert_equal "", SchematizedJson.read_raw(doc, "c")
  end

  def test_render_object_round_trips_and_sorts
    assert_equal "{\"a\":\"x\",\"b\":1}",
      SchematizedJson.render_object(SchematizedJson.parse_object("{\"b\":1,\"a\":\"x\"}"))
  end

  def test_decode_string_unescapes
    assert_equal "a\"b", SchematizedJson.decode_string("\"a\\\"b\"")
    assert_equal "a\nb", SchematizedJson.decode_string("\"a\\nb\"")
    assert_equal "a\\b", SchematizedJson.decode_string("\"a\\\\b\"")
  end

  # ── read_* ─────────────────────────────────────────────────────

  def test_read_boolean
    doc = "{\"on\":true,\"off\":false,\"void\":null}"
    assert_equal true, SchematizedJson.read_boolean(doc, "on", false)
    assert_equal false, SchematizedJson.read_boolean(doc, "off", true)
    # A stored null and an absent key both answer the declared default.
    assert_equal true, SchematizedJson.read_boolean(doc, "void", true)
    assert_equal true, SchematizedJson.read_boolean(doc, "missing", true)
    assert_equal false, SchematizedJson.read_boolean(nil, "on", false)
  end

  def test_read_integer
    doc = "{\"n\":42,\"neg\":-7}"
    assert_equal 42, SchematizedJson.read_integer(doc, "n", 0)
    assert_equal(-7, SchematizedJson.read_integer(doc, "neg", 0))
    assert_equal 5, SchematizedJson.read_integer(doc, "missing", 5)
  end

  def test_read_string
    doc = "{\"greeting\":\"hello\"}"
    assert_equal "hello", SchematizedJson.read_string(doc, "greeting", "hi")
    assert_equal "hi", SchematizedJson.read_string(doc, "missing", "hi")
  end

  # ── write_* ────────────────────────────────────────────────────

  def test_write_into_an_empty_column
    assert_equal "{\"on\":true}", SchematizedJson.write_boolean(nil, "on", true)
    assert_equal "{\"n\":42}", SchematizedJson.write_integer("", "n", 42)
    assert_equal "{\"s\":\"hi\"}", SchematizedJson.write_string("{}", "s", "hi")
  end

  def test_write_preserves_untouched_keys_and_sorts
    assert_equal "{\"a\":\"x\",\"b\":1,\"c\":false}",
      SchematizedJson.write_boolean("{\"b\":1,\"a\":\"x\"}", "c", false)
  end

  def test_write_replaces_an_existing_key
    assert_equal "{\"on\":false}",
      SchematizedJson.write_boolean("{\"on\":true}", "on", false)
  end

  def test_write_escapes_the_value
    assert_equal "{\"s\":\"a\\\"b\"}", SchematizedJson.write_string("{}", "s", "a\"b")
  end

  def test_write_then_read_round_trips
    doc = SchematizedJson.write_string(
      SchematizedJson.write_boolean(nil, "on", true), "s", "a\"b"
    )
    assert_equal true, SchematizedJson.read_boolean(doc, "on", false)
    assert_equal "a\"b", SchematizedJson.read_string(doc, "s", "")
  end
end
