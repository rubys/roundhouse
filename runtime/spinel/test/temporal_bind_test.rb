# A TIME bind rendered into SQL text must use the same format the
# temporal COLUMN is stored in.
#
# Minitest-shaped: a CRuby-only framework test, quarantined out of the
# spin shape by `project.rs::spin_shape` (a `Minitest::Test` subclass is
# CRuby-only regardless), same as dom_test.rb and route_helpers_test.rb
# beside it.
#
# WHY THIS FILE EXISTS. The adapter composes SQL by inlining escaped
# values rather than binding parameters (see sqlite_adapter.rb's
# header), so a temporal comparison is string-vs-string and a bind
# written in ANY other format answers the wrong rows SILENTLY — no
# error, no empty result to notice, just a different set. `Time#to_s`
# is exactly that other format: local time with a zone offset
# (`2026-08-22 15:30:00 -0400`) against a column holding UTC with
# microseconds (`2026-08-22 19:30:00.123456`). The two agree on the
# date and disagree at the HOUR, so campfire's
# `where("created_at < ?", message.created_at)` matched NOTHING and its
# `>` twin matched EVERYTHING — a message list that paged backwards
# into an empty page and forwards into the whole room.
require "minitest/autorun"
require_relative "test_helper"

class TemporalBindTest < Minitest::Test
  # A time with a fractional part and a non-UTC zone: both halves of
  # the bug (the offset and the missing microseconds) are visible.
  def a_time
    Time.at(1_700_000_000.123456).localtime("-04:00")
  end

  def test_a_time_bind_uses_the_column_writer
    t = a_time
    assert_equal "'" + ActiveSupport.format_db_time(t).to_s + "'",
                 SqliteAdapter.escape_value(t)
  end

  # The property that matters, stated directly: what a WHERE bind
  # writes and what a column write stores are the same text, so `<`
  # and `>` mean what they say.
  def test_the_bind_and_the_stored_column_agree
    t = a_time
    stored = ActiveSupport.format_db_time(t).to_s
    assert_equal "'#{stored}'", SqliteAdapter.escape_value(t)
    assert_includes stored, "."
    refute_includes stored, "-04:00"
  end

  # The other branches are untouched — this is an added arm, not a
  # rewrite of the escaper.
  def test_the_other_value_kinds_are_unchanged
    assert_equal "42", SqliteAdapter.escape_value(42)
    assert_equal "1", SqliteAdapter.escape_value(true)
    assert_equal "0", SqliteAdapter.escape_value(false)
    assert_equal "NULL", SqliteAdapter.escape_value(nil)
    assert_equal "'it''s'", SqliteAdapter.escape_value("it's")
  end
end
