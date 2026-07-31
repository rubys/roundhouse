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
  def self.parse_db_time(str)
    return nil if str.nil?
    return nil if str.length < 19
    Time.utc(
      str[0, 4].to_i, str[5, 2].to_i, str[8, 2].to_i,
      str[11, 2].to_i, str[14, 2].to_i, str[17, 2].to_i
    )
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
  # This spinel-subset version is second-granularity UTC, because
  # `parse_db_time` above is: no sub-second read, no local-zone
  # conversion. That is the gap to close when spinel's Time grows those
  # — not something for the JSON serializer to work around. Nothing
  # exercises it yet: the spinel tree emits controllers with
  # `format_breadth=false`, so its rss/json respond_to arms don't
  # render. The CRuby/JRuby overlay's sibling file shadows this one
  # (dedupe last-wins) with the exact implementation, and that IS the
  # lane the /hottest parity route measures.
  def self.json_time(str)
    t = parse_db_time(str)
    return nil if t.nil?
    "#{t.strftime("%Y-%m-%dT%H:%M:%S")}.000+00:00"
  end

  # Normalize a temporal-writer value into the canonical storage form.
  # Time → stamped (same shape as db_now); nil → nil (nullable column
  # cleared: `self.banned_at = nil`); String passes through untouched.
  # The synthesized model writers (`banned_at=`) route every store
  # through this so column TEXT stays homogeneous and lexicographically
  # ordered.
  # RFC 2822 date, the shape stdlib `time` gives `Time#rfc2822` — which
  # spinel has no `time` package to provide and which cannot be added by
  # reopening `Time` (a reopened built-in loses its own method table for
  # self-calls). Composed from strftime instead, whose `%a`/`%b` are the
  # English abbreviations RFC 2822 requires on every locale.
  #
  # The zone tail is the CONSTANT "-0000" here, not stdlib's
  # `utc? ? "-0000" : <offset>` conditional, because spinel's Time
  # carries no zone model to branch on: `utc?`, `zone` and `utc_offset`
  # are all undefined on it, and there is no `getlocal`. Every Time on
  # this tree is therefore the UTC one `parse_db_time` built, and
  # "-0000" — RFC 2822's "UTC, no local zone information" marker — is
  # what stdlib renders for exactly that. The overlay twin keeps the
  # conditional, since CRuby's Time does carry a zone.
  #
  # This is the same missing zone model that leaves the AOT lane's
  # rendered timestamps at +0000 where the CRuby lane applies the app's
  # `config.time_zone`; when spinel grows one, this reverts to the
  # stdlib rule and both follow from the same fix.
  def self.rfc2822(t)
    return nil if t.nil?
    t.strftime("%a, %d %b %Y %H:%M:%S ") + "-0000"
  end

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
