# CRuby-only UTC-safe datetime-column parsing.
#
# Rails' sqlite3 adapter stores Date/DateTime/Time columns as bare
# `YYYY-MM-DD HH:MM:SS[.ffffff]` TEXT with no zone marker — always
# implicitly UTC. Ruby's stdlib `Time.parse`, when a string carries no
# zone marker, defaults to the *system's local zone* instead (a
# well-known gotcha) — silently shifting every such value by the host's
# UTC offset. `apply_datetime_lowering`'s synthesized column reader
# calls `parse_db_time` instead of bare `Time.parse` so parsing is
# correct regardless of the machine's `TZ`. A string that already
# carries an explicit zone (e.g. `fill_timestamps`' `Time.now.utc.iso8601`,
# which appends "Z") is left alone — that marker is authoritative and
# must not be overridden.
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
end
