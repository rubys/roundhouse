# Native date-only parsing plus UTC-safe timestamp parsing/stamping.
#
# Dates store as YYYY-MM-DD, with no time or zone. Separately,
# Rails' sqlite3 adapter stores DateTime/Time columns as bare
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
# Stdlib `time` for `Time.parse`. Safe to require here: this file is
# CRuby/JRuby-overlay-only (never part of the spinel AOT require walk),
# and requiring it locally keeps every bootstrap that chains this file
# (main.rb, or test_helper via runtime/db.rb) self-sufficient.
require "time"
require "date"

module ActiveSupport
  # Date-only storage has no clock or zone. Native Date is the authority
  # for parsing and calendar arithmetic (including >>'s one-shot clamp).
  def self.parse_db_date(str)
    return nil if str.nil? || str.empty?
    Date.iso8601(str)
  end

  def self.format_db_date(value)
    return nil if value.nil?
    return nil if value.is_a?(String) && value.empty?
    date = case value
    when Date then value.to_date
    when String then Date.iso8601(value)
    else raise TypeError, "expected Date or ISO date text"
    end
    date.iso8601
  end

  # Date calendar helpers (`date_*`) live only in
  # `active_support_date_parsing.rb` (date-gated package). Do not
  # redefine them here — CRuby boot re-injects that file after overlay.

  # Rails zone name → IANA identifier (the ActiveSupport::TimeZone::
  # MAPPING subset corpora have needed; extend as apps demand). Names
  # not listed pass through unchanged — a valid IANA string works
  # as-is in TZ. Consumed by main.rb's boot-time ENV["TZ"] pin.
  RAILS_TZ_TO_IANA = {
    "UTC" => "UTC",
    "Eastern Time (US & Canada)" => "America/New_York",
    "Central Time (US & Canada)" => "America/Chicago",
    "Mountain Time (US & Canada)" => "America/Denver",
    "Pacific Time (US & Canada)" => "America/Los_Angeles",
    "Arizona" => "America/Phoenix",
    "Hawaii" => "Pacific/Honolulu",
    "Alaska" => "America/Anchorage",
    "London" => "Europe/London",
    "Paris" => "Europe/Paris",
    "Berlin" => "Europe/Berlin",
    "Tokyo" => "Asia/Tokyo",
    "Sydney" => "Australia/Sydney",
  }.freeze

  def self.parse_db_time(str)
    return nil if str.nil? || str.empty?
    # Fast path for the sqlite3 adapter's own storage form — bare
    # "YYYY-MM-DD HH:MM:SS[.ffffff]", no zone marker, implicitly UTC —
    # which is what every hydrated column carries. `Time.parse`'s
    # `Date._parse` regex machinery dominates timestamp-heavy renders
    # (≈9% of /comments wall); a fixed-format `Time.utc` extraction is
    # the same instant at a fraction of the cost, matching what Rails'
    # own adapter does. `Time.parse` stays the fallback for the rare
    # zone-carrying string (API-supplied, or a pre-`db_now` build's "Z").
    if (m = /\A(\d{4})-(\d\d)-(\d\d)[ T](\d\d):(\d\d):(\d\d)(?:\.(\d+))?\z/.match(str))
      usec = m[7] ? "#{m[7]}000000"[0, 6].to_i : 0
      return ActiveSupport.present(Time.utc(m[1].to_i, m[2].to_i, m[3].to_i, m[4].to_i, m[5].to_i, m[6].to_i, usec))
    end
    t = str =~ /(Z|[+-]\d\d:?\d\d)\z/ ? Time.parse(str) : Time.parse("#{str} UTC")
    # Present in the app's zone, exactly as ActiveRecord returns
    # TimeWithZone values in Time.zone: main.rb pins ENV["TZ"] to the
    # app's config.time_zone (default UTC — Rails' default — so
    # rendered offsets never follow the HOST's zone). Instants are
    # unchanged; only strftime/iso8601 presentation moves.
    ActiveSupport.present(t)
  end

  # A temporal column as JSON. Rails serializes an AR temporal value
  # through `TimeWithZone#as_json`, which is `xmlschema(3)` — ISO8601
  # with exactly three fractional digits and the app zone's offset
  # (`2023-05-08T05:28:49.595-05:00`). The stored TEXT is neither
  # (`2023-05-08 10:28:49.595725`, UTC, six digits), so serializing the
  # raw column value is a different document; /hottest's JSON is a
  # parity route and locks these bytes.
  #
  # `parse_db_time` above already lands the instant in the app's zone,
  # so `xmlschema(3)` is the whole formatting rule.
  def self.json_time(str)
    t = parse_db_time(str)
    return nil if t.nil?
    t.xmlschema(3)
  end

  # ---- THE TEST CLOCK ------------------------------------------------
  #
  # `ActiveSupport.now` is the ONE time read in this runtime. Every
  # other one calls it — `db_now` below, `Duration#ago`/`#from_now`
  # beside it — so `travel_to` in the test harness can move the clock
  # for the whole app with a single write, and a model does not have to
  # know it is under test.
  #
  # ActiveSupport's own `TimeHelpers` do this by stubbing `Time.now`,
  # which needs a reopened built-in; the shared runtime cannot reopen
  # one, and spinel AOT cannot stub at all. A module function every
  # caller already routes through is the same seam without either.
  #
  # An ARRAY, not a module ivar: `TRANSPORTS` in broadcasts.rb holds
  # mutable module state the same way, and the literal `[0]` is what
  # pins the element type for spinel.
  #
  # PRODUCTION IS ZERO. Nothing outside the harness ever calls
  # `travel`, so `now` is `Time.now` plus a constant-folded 0.
  TRAVEL_OFFSET = [0]

  def self.travel_offset
    TRAVEL_OFFSET[0]
  end

  # Whole seconds relative to the real clock. `travel(0)` is Rails'
  # `travel_back`, which the harness runs after every test.
  def self.travel(seconds)
    TRAVEL_OFFSET.clear
    TRAVEL_OFFSET << seconds
  end

  def self.now
    ActiveSupport.present(Time.now + TRAVEL_OFFSET[0])
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
    t = ActiveSupport.now.getutc
    format(
      "%04d-%02d-%02d %02d:%02d:%02d.%06d",
      t.year, t.month, t.day, t.hour, t.min, t.sec, t.usec
    )
  end

  # Stdlib is the authority here (this tree already requires "time"),
  # so the overlay delegates rather than re-derives — the CRuby lane's
  # feed bytes are exactly what they were before the call site was
  # grounded to a module function. The spinel twin composes the same
  # shape from strftime and pins the zone tail; see its comment for why
  # the two differ.
  def self.rfc2822(t)
    return nil if t.nil?
    t.rfc2822
  end

  # Normalize a temporal-writer value into the canonical storage form
  # above. Time → stamped; nil → nil (a nullable column being cleared:
  # `self.banned_at = nil`); String passes through untouched (already
  # canonical, or carries its own zone marker — same trust the readers
  # give the column). The synthesized model writers (`banned_at=`)
  # route every store through this so column TEXT stays homogeneous
  # and lexicographically ordered.
  def self.format_db_time(value)
    return nil if value.nil?
    if value.is_a?(Time)
      t = value.getutc
      return format(
        "%04d-%02d-%02d %02d:%02d:%02d.%06d",
        t.year, t.month, t.day, t.hour, t.min, t.sec, t.usec
      )
    end
    value
  end

  def self.parse_time(str)
    Time.parse(str, Time.now + TRAVEL_OFFSET[0])
  end

  # Not `Time.parse`: ActiveSupport's `TimeZone#parse` answers nil for no date and lands an offset in the app's zone.
  def self.zone_parse(str)
    parts = Date._parse(str, false)
    return nil if parts.empty?
    return ActiveSupport.present(Time.at(parts[:seconds] + parts.fetch(:sec_fraction, 0))) if parts[:seconds]
    now = ActiveSupport.now
    year = parts.fetch(:year, now.year)
    mon = parts.fetch(:mon, now.month)
    mday = parts.fetch(:mday, parts[:year] || parts[:mon] ? 1 : now.day)
    sec = parts.fetch(:sec, 0) + parts.fetch(:sec_fraction, 0)
    unless parts[:offset]
      return ActiveSupport.local_time(year, mon, mday, parts.fetch(:hour, 0), parts.fetch(:min, 0), sec.to_i, ((sec - sec.to_i) * 1_000_000_000).to_i)
    end
    ActiveSupport.present(Time.new(year, mon, mday, parts.fetch(:hour, 0), parts.fetch(:min, 0), sec, parts[:offset]))
  end
end
