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
