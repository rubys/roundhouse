# CRuby/JRuby-only Time-aware JSON datetime encoding.
#
# On the Ruby/spinel trees, a Date/DateTime/Time column's accessor
# returns a real `Time` object (see `apply_datetime_lowering`), so the
# value the Jbuilder lowerer hands to `JsonBuilder.encode_datetime` is a
# `Time`, not the raw stored string. The shared
# `runtime/ruby/json_builder.rb` deliberately stays `Time`-free (it
# transpiles to targets that have no `Time` type), so it can only format
# the stored TEXT form; this overlay shadows `encode_datetime` for the
# Ruby tree with a version that formats the `Time` directly.
#
# `getutc` (non-mutating) + `iso8601(3)` reproduces Rails' millisecond-
# precision JSON time format exactly. Required after
# `runtime/json_builder` in main.rb so this definition wins.
module JsonBuilder
  def self.encode_datetime(s)
    return "null" if s.nil?
    return "\"#{s.getutc.iso8601(3)}\"" if s.is_a?(Time)
    # Defensive: a datetime value that never went through the lowered
    # Time accessor (a raw stored String) still serializes as plain
    # quoted text rather than raising.
    "\"#{encode_string(s.to_s)}\""
  end
end
