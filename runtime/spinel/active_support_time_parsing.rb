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
    Time.utc(
      str[0, 4].to_i, str[5, 2].to_i, str[8, 2].to_i,
      str[11, 2].to_i, str[14, 2].to_i, str[17, 2].to_i
    ).getlocal
  end

  def self.db_now
    t = Time.now.utc
    f = t.to_f
    micros = ((f - f.to_i) * 1_000_000).to_i
    format(
      "%04d-%02d-%02d %02d:%02d:%02d.%06d",
      t.year, t.mon, t.mday, t.hour, t.min, t.sec, micros
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
  # Still second-granularity (no sub-second read), so the fraction is
  # literal ".000". Nothing on this tree exercises it yet: the spinel
  # emit drops the json respond_to arm, so /hottest renders HTML here
  # where Rails renders JSON — see src/lower/controller/body.rs.
  def self.json_time(str)
    t = parse_db_time(str)
    return nil if t.nil?
    t.strftime("%Y-%m-%dT%H:%M:%S.000%:z")
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
      f = t.to_f
      micros = ((f - f.to_i) * 1_000_000).to_i
      return format(
        "%04d-%02d-%02d %02d:%02d:%02d.%06d",
        t.year, t.mon, t.mday, t.hour, t.min, t.sec, micros
      )
    end
    value
  end
end
