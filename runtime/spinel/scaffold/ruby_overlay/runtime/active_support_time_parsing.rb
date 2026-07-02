# CRuby-only UTC-safe datetime-column parsing + storage-form stamping.
#
# Rails' sqlite3 adapter stores Date/DateTime/Time columns as bare
# `YYYY-MM-DD HH:MM:SS[.ffffff]` TEXT with no zone marker — always
# implicitly UTC. Ruby's stdlib `Time.parse`, when a string carries no
# zone marker, defaults to the *system's local zone* instead (a
# well-known gotcha) — silently shifting every such value by the host's
# UTC offset. `apply_datetime_lowering`'s synthesized column reader
# calls `parse_db_time` instead of bare `Time.parse` so parsing is
# correct regardless of the machine's `TZ`. A string that already
# carries an explicit zone (an API-supplied value, or a timestamp
# written by a pre-`db_now` roundhouse build, which appended "Z") is
# left alone — that marker is authoritative and must not be overridden.
#
# Nil-safe: a NULL / unset column hydrates as `nil` or `""` (the
# adapter's `column_text` maps SQL NULL to `""`), and the synthesized
# reader calls this unguarded — absent storage must yield `nil`, not an
# ArgumentError out of `Time.parse("")`.
module ActiveSupport
  def self.parse_db_time(str)
    return nil if str.nil? || str.empty?
    str =~ /(Z|[+-]\d\d:?\d\d)\z/ ? Time.parse(str) : Time.parse("#{str} UTC")
  end

  # Write-side sibling of `parse_db_time`: current UTC time in Rails'
  # exact storage form — "YYYY-MM-DD HH:MM:SS.ffffff", space separator,
  # zero-padded 6-digit fractional seconds, no zone marker (implicitly
  # UTC, matching what Rails' sqlite3 adapter writes byte-for-byte).
  # `fill_timestamps` stamps with this so a column's TEXT values stay
  # homogeneous — and lexicographically ordered — when a
  # roundhouse-emitted app shares a database with a real Rails app.
  # `getutc` (non-mutating); sprintf over strftime's `%6N` because
  # plain integer fields are the most portable surface across
  # CRuby/JRuby.
  def self.db_now
    t = Time.now.getutc
    format(
      "%04d-%02d-%02d %02d:%02d:%02d.%06d",
      t.year, t.month, t.day, t.hour, t.min, t.sec, t.usec
    )
  end
end
