# Roundhouse Crystal datetime runtime — the native-`Time` seam for
# temporal (Date/DateTime/Time) columns.
#
# Storage stays portable ISO-8601 TEXT: a temporal column hydrates into a
# `String` ivar (`column_text`), exactly like every other target. The
# model's synthesized reader parses that text into a native `Time` via
# `Roundhouse::DateTime.parse` (see `src/emit/crystal/expr.rs`, which maps
# the `ActiveSupport.parse_db_time` intrinsic here). JSON serialization
# then formats a `Time` back to Rails' canonical `...Z` millisecond form
# via the `JsonBuilder.encode_datetime(Time?)` overload below.

module Roundhouse
  module DateTime
    # Parse a stored ISO-8601 value into a native UTC `Time`. Nil-safe:
    # nil / empty → nil. Handles the two forms roundhouse ever stores:
    #
    #   * DB-dump / seed form — "2026-05-15 21:14:56.300213" (space
    #     separator, zone-less, microsecond precision, implicitly UTC).
    #   * RFC3339 form — "2026-05-15T21:14:56Z" (what `fill_timestamps`
    #     writes via `Time.utc.to_rfc3339`, and API-supplied values).
    #
    # A value that parses under neither returns nil rather than raising —
    # a malformed stored timestamp shouldn't take down a read path.
    def self.parse(s : String?) : Time?
      return nil if s.nil?
      str = s
      return nil if str.empty?
      if str.size > 10 && str[10] == ' '
        fmt = str.includes?('.') ? "%F %H:%M:%S.%N" : "%F %H:%M:%S"
        begin
          return Time.parse(str, fmt, Time::Location::UTC)
        rescue
          return nil
        end
      end
      begin
        Time.parse_rfc3339(str)
      rescue
        nil
      end
    end
  end
end

module JsonBuilder
  # `Time` overload of `encode_datetime`. The transpiled `String?` version
  # (json_builder.cr, from the shared runtime) handles pre-formatted text;
  # this one formats a native `Time` — what a temporal column's reader
  # yields — to Rails' canonical JSON shape: UTC, millisecond precision,
  # `Z` suffix (e.g. "2026-05-15T21:14:56.300Z"). The compare harness
  # canonicalizes Rails' microsecond precision down to milliseconds, so
  # this matches byte-for-byte.
  def self.encode_datetime(t : Time?) : String
    return "null" if t.nil?
    "\"#{t.to_utc.to_rfc3339(fraction_digits: 3)}\""
  end
end
