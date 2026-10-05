# Spinel-subset temporal intrinsics: ActiveSupport.parse_db_time /
# db_now. Sibling of the CRuby/JRuby overlay's
# ruby_overlay/runtime/active_support_time_parsing.rb, which shadows
# this file on those trees (dedupe last-wins) with the stdlib-backed
# implementation. This one avoids everything spinel's Time lacks —
# no `require "time"`, no `Time.parse`, no `usec` reader.
#
# The synthesized temporal-column readers call `parse_db_time`
# (apply_datetime_lowering runs on the spinel-shape emit too) and
# `Base#save`'s fill_timestamps calls `db_now`. Until spinel's
# unresolved-call gate turned strict (spinel 1356cb14), both calls
# silently no-op'd here — readers returned nil, stamps were skipped —
# because the spinel tree simply lacked the module (spinel#1661).
#
# Storage form is Rails' fixed-width "YYYY-MM-DD HH:MM:SS[.ffffff]"
# TEXT (implicitly UTC, `T` tolerated as the separator), so a
# positional parse is exact. Sub-second storage survives writes
# (db_now stamps it via Time#to_f) but truncates on read —
# `Time.utc` takes whole seconds; comparisons and strftime in the
# corpus are second-granularity, and JSON serializes from the raw
# string (`<col>_raw`), not the parsed Time.
module ActiveSupport
  # SQL DATE has no clock or zone. The empty string is the SQLite
  # adapter's nil representation for a nullable column.
  def self.parse_db_date(value)
    return nil if value.nil? || value == ""
    Date.iso8601(value)
  end

  def self.format_db_date(value)
    return nil if value.nil?
    return nil if value.is_a?(String) && value == ""
    return value.iso8601 if value.is_a?(Date)
    return Date.iso8601(value).iso8601 if value.is_a?(String)
    raise TypeError, "expected Date, String, or nil"
  end

  # Rails zone name → IANA identifier. TWIN of the constant in the
  # CRuby/JRuby overlay's sibling file, which shadows this whole file on
  # those trees — the two must agree, so extend both together. Names not
  # listed pass through unchanged, since a valid IANA string works as-is
  # in TZ. Consumed by main.rb's boot-time ENV["TZ"] pin.
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

  # Hydrate the stored UTC instant, then land it in the app's zone —
  # Rails presents every AR temporal value in `config.time_zone`
  # REGARDLESS of the host's zone, and main.rb has pinned ENV["TZ"] to
  # that zone before any render, so `getlocal` resolves against it.
  # Doing the shift HERE rather than at each render site is what makes
  # strftime, iso8601 and pubDate agree without every call site knowing
  # about zones — the same seam the CRuby overlay's twin uses.
  #
  # The offset is DST-correct because libc resolves the instant against
  # the host's tzdata: America/Chicago is -0500 in July and -0600 in
  # January. Nothing is baked at compile time except the zone NAME.
  def self.parse_db_time(str)
    return nil if str.nil?
    return nil if str.length < 19
    # The stored TEXT carries fractional seconds — Rails' datetime(6)
    # writes ".418418" and so does our own db_now — and the CRuby
    # overlay's twin reads them (usec, via its regex). Reading whole
    # seconds only truncated every hydrated timestamp: campfire's
    # `updated_at.to_fs(:epoch)` answered `…596000` where Rails said
    # `…596418`, found by the room-page compare gate on the binary.
    # Fixed-position slicing with a digit walk, same no-regex rationale
    # as the fields above; digits beyond micros are ignored, a
    # non-digit ends the read (so a legacy "…16Z" contributes nothing).
    # Plain reassignment, not `+=` — spinel has no operator-assignment
    # node for locals (LocalVariableOperatorWriteNode is refused).
    micros = 0
    if str.length > 20 && str[19, 1] == "."
      digits = 0
      while digits < 6
        c = str[20 + digits, 1]
        break if c.nil? || c.empty? || c < "0" || c > "9"
        digits = digits + 1
      end
      if digits > 0
        micros = "#{str[20, digits]}000000"[0, 6].to_i
      end
    end
    # The micros go in as `Time.utc`'s seventh argument, NOT as a float
    # added afterwards. `t + 170926 / 1_000_000.0` lands at
    # .170925999, and `format_db_time` below then truncates it back to
    # .170925 — so a `created_at > ?` bound from a hydrated record
    # matched the record itself, and campfire's "page after" test
    # passed or failed on which microsecond the fixtures loaded in.
    t = Time.utc(
      str[0, 4].to_i, str[5, 2].to_i, str[8, 2].to_i,
      str[11, 2].to_i, str[14, 2].to_i, str[17, 2].to_i, micros
    )
    ActiveSupport.present(t)
  end

  def self.db_now
    t = ActiveSupport.now.utc
    format(
      "%04d-%02d-%02d %02d:%02d:%02d.%06d",
      t.year, t.mon, t.mday, t.hour, t.min, t.sec, t.usec
    )
  end

  # A temporal column as JSON: Rails serializes one through
  # `TimeWithZone#as_json` → `xmlschema(3)`, i.e. ISO8601 with exactly
  # three fractional digits and the app zone's offset
  # (`2023-05-08T05:28:49.595-05:00`), NOT the stored TEXT.
  #
  # `parse_db_time` above now lands the instant in the app's zone, so
  # the offset is the receiver's own — NOT a hardcoded "+00:00", which
  # would label local clock fields as UTC and be wrong by the offset.
  # The milliseconds are `usec / 1000`, formatted apart: strftime here
  # has no millisecond directive, and xmlschema(3) TRUNCATES (.027418 →
  # .027), which integer division does too. A monomorphized `as_json`
  # writer (lower::as_json_writer, `PairEncoding::ZonedTime`) is what
  # reaches this — lobsters' /hottest, one call per story.
  def self.json_time(str)
    t = parse_db_time(str)
    return nil if t.nil?
    t.strftime("%Y-%m-%dT%H:%M:%S") + format(".%03d", t.usec / 1000) + t.strftime("%:z")
  end

  # RFC 2822 date, the shape stdlib `time` gives `Time#rfc2822` — which
  # spinel has no `time` package to provide and which cannot be added by
  # reopening `Time` (a reopened built-in loses its own method table for
  # self-calls). Composed from strftime instead, whose `%a`/`%b` are the
  # English abbreviations RFC 2822 requires on every locale.
  #
  # The zone tail is the receiver's own offset, via `%z`.
  #
  # NOT stdlib's `utc? ? "-0000" : <offset>` conditional, for two
  # reasons. Reachability: every value that gets here came from
  # `parse_db_time`, which ends in `.getlocal`, so the receiver is always
  # the host-local kind and `utc?` is always false — including when the
  # app declares no `config.time_zone` and the pin lands on "UTC", where
  # a local-kind Time at offset 0 renders "+0000" and stdlib agrees.
  # Typing: `utc?` is not reachable on this receiver anyway — the RBS
  # types the parameter `Time?`, and calling it raises NoMethodError at
  # runtime on the boxed value (measured: /rss 500s).
  #
  # The divergence this leaves is a caller handing us a true `Time.utc`,
  # which would render "+0000" where stdlib renders "-0000". No such
  # caller exists; the RSS feed reads `story.created_at`.
  #
  # This used to be a hardcoded "-0000" on the belief that spinel's Time
  # had no zone model to branch on. It does — `utc?`, `zone`,
  # `utc_offset` and `getlocal` all exist, and sp_Time carries a 3-state
  # zone kind, added 2026-07 in matz/spinel 1a7c3597 + fb6e6685. The last
  # real blocker was that strftime and iso8601 rendered UTC clock fields
  # for a fixed-offset Time, so `getlocal` output could not be trusted;
  # fixed by matz/spinel#3492, merged 2026-08-01 as 53feb9df.
  def self.rfc2822(t)
    return nil if t.nil?
    t.strftime("%a, %d %b %Y %H:%M:%S %z")
  end

  # Normalize a temporal-writer value into the canonical storage form.
  # Time → stamped (same shape as db_now); nil → nil (nullable column
  # cleared: `self.banned_at = nil`); String passes through untouched.
  # The synthesized model writers (`banned_at=`) route every store
  # through this so column TEXT stays homogeneous and lexicographically
  # ordered.
  def self.format_db_time(value)
    return nil if value.nil?
    if value.is_a?(Time)
      t = value.utc
      # `usec`, not float arithmetic on `to_f`: a double near 2e9
      # seconds resolves to ~0.4 µs, so `((f - f.to_i) * 1e6).to_i`
      # lands a microsecond short of what `parse_db_time` read.
      return format(
        "%04d-%02d-%02d %02d:%02d:%02d.%06d",
        t.year, t.mon, t.mday, t.hour, t.min, t.sec, t.usec
      )
    end
    value
  end

  def self.parse_time(str)
    t = parse_fields(str, false)
    raise ArgumentError, "no time information in #{str.inspect}" if t.nil?
    t
  end

  def self.zone_parse(str)
    parse_fields(str, true)
  end

  # Not `Date._parse`'s every shape: a string outside these forms raises rather than parse to a different instant.
  def self.parse_fields(str, in_zone)
    raise TypeError, "no implicit conversion of nil into String" if str.nil?
    s = str.strip
    m = /\A(\d{4})[-\/](\d{1,2})[-\/](\d{1,2})(?:(?:T|\s+)(\d{1,2}):(\d{2})(?::(\d{2})(?:\.(\d+))?)?)?\s*(Z|UTC|GMT|[-+]\d{2}:?\d{2})?\z/i.match(s)
    return build_time(in_zone, m[1].to_i, m[2].to_i, m[3].to_i, m[4], m[5], m[6], m[7], m[8]) if m
    m = /\A(\d{4})(\d{2})(\d{2})\z/.match(s)
    return build_time(in_zone, m[1].to_i, m[2].to_i, m[3].to_i, nil, nil, nil, nil, nil) if m
    m = /\A(\d{1,2})\/(\d{1,2})\z/.match(s)
    return build_time(in_zone, -1, m[1].to_i, m[2].to_i, nil, nil, nil, nil, nil) if m
    m = /\A(\d{1,2}):(\d{2})(?::(\d{2})(?:\.(\d+))?)?\s*(Z|UTC|GMT|[-+]\d{2}:?\d{2})?\z/i.match(s)
    return build_time(in_zone, -1, -1, -1, m[1], m[2], m[3], m[4], m[5]) if m
    m = /\A(?:(?:sun|mon|tue|wed|thu|fri|sat)[a-z]*,?\s+)?(?:(\d{1,2})\s+)?(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?(?:\s+(\d{1,2})(?!\d),?)?(?:\s+(\d{4}))?(?:\s+(\d{1,2}):(\d{2})(?::(\d{2})(?:\.(\d+))?)?)?\s*(Z|UTC|GMT|[-+]\d{2}:?\d{2})?\z/i.match(s)
    if m
      mon = %w[jan feb mar apr may jun jul aug sep oct nov dec].index(m[2].downcase).to_i + 1
      day = m[1] ? m[1].to_i : (m[3] ? m[3].to_i : -1)
      year = m[4] ? m[4].to_i : -1
      return build_time(in_zone, year, mon, day, m[5], m[6], m[7], m[8], m[9])
    end
    return nil unless s =~ /\d|jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec|sun|mon|tue|wed|thu|fri|sat/i
    raise ArgumentError, "unsupported time format: #{str.inspect}"
  end

  # -1 marks a missing date part, filled from `now` the way ActiveSupport's `parts_to_time` does.
  def self.build_time(in_zone, year, mon, mday, hour_s, min_s, sec_s, frac, zone)
    now = ActiveSupport.now
    y = year < 0 ? now.year : year
    mo = mon < 0 ? now.mon : mon
    d = mday
    d = (year >= 0 || mon >= 0) ? 1 : now.mday if d < 0
    hour = hour_s ? hour_s.to_i : 0
    min = min_s ? min_s.to_i : 0
    sec = sec_s ? sec_s.to_i : 0
    usec = frac ? "#{frac}000000"[0, 6].to_i : 0
    raise ArgumentError, "argument out of range" if mo < 1 || mo > 12 || d < 1 || d > 31
    raise ArgumentError, "argument out of range" if hour > 24 || min > 59 || sec > 60
    # Not the zone's clock for `Time.parse`: Rails reads a zoneless string in the system zone there.
    return (in_zone ? ActiveSupport.local_time(y, mo, d, hour, min, sec, usec * 1000) : Time.local(y, mo, d, hour, min, sec, usec)) if zone.nil?
    z = zone.upcase
    t = Time.utc(y, mo, d, hour, min, sec, usec)
    return (in_zone ? ActiveSupport.present(t) : t) if z == "Z" || z == "UTC" || z == "GMT"
    digits = z.delete(":")
    offset = digits[1, 2].to_i * 3600 + digits[3, 2].to_i * 60
    offset = -offset if digits[0] == "-"
    shifted = t - offset
    in_zone ? ActiveSupport.present(shifted) : shifted.getlocal(offset)
  end
end
