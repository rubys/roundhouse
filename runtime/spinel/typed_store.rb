# typed_store gem subset (spinel twin of the CRuby overlay's
# runtime/typed_store.rb, which parses with the real YAML) — the
# settings column holds a FLAT scalar hash and nothing else in the
# benchmark DB:
#
#   ---
#   prefers_color_scheme: system
#   email_notifications: false
#   homepage: http://…
#
# so this parser covers exactly that subset: `key: value` lines at
# zero indentation, scalar values (string / true / false / integer /
# empty→nil). Indented or list lines (a structured value like a
# keybase proof map) are SKIPPED — their attribute reads fall back to
# the declared default, and a write re-serializes only the flat keys.
# Reads far outnumber writes (the benchmark never writes settings), so
# `parse` keeps a one-entry cache keyed by the serialized string —
# /settings reads ~24 attributes off the same row without re-parsing
# per attribute. Same shape as the CRuby overlay twin.
module TypedStore
  @cache_key = nil
  @cache_val = {}

  def self.parse(serialized)
    s = serialized.to_s
    return @cache_val if s == @cache_key
    h = {}
    lines = s.split("\n")
    i = 0
    while i < lines.length
      line = lines[i].to_s
      i += 1
      next if line == "---" || line == ""
      # Indented / list / comment lines: structured values, skipped.
      c0 = line[0, 1].to_s
      next if c0 == " " || c0 == "-" || c0 == "#"
      sep = line.index(": ")
      if sep.nil?
        # `key:` with no value — an explicit empty scalar (nil).
        if line.end_with?(":")
          h[line[0, line.length - 1].to_s] = nil
        end
        next
      end
      key = line[0, sep].to_s
      raw = line[sep + 2, line.length].to_s
      h[key] = if raw == "true"
        true
      elsif raw == "false"
        false
      elsif raw == raw.to_i.to_s
        raw.to_i
      elsif raw.length >= 2 && ((raw[0, 1] == "\"" && raw[raw.length - 1, 1] == "\"") ||
                                (raw[0, 1] == "'" && raw[raw.length - 1, 1] == "'"))
        raw[1, raw.length - 2].to_s
      else
        raw
      end
    end
    @cache_key = s
    @cache_val = h
    h
  end

  # The stored value for `key`, or `default` when the hash carries no
  # entry (matching typed_store: an explicit stored nil stays nil).
  def self.read(serialized, key, default)
    h = parse(serialized)
    h.key?(key) ? h[key] : default
  end

  # Returns the re-serialized store with `key` set — the caller
  # assigns it back to the column ivar. Flat keys only (see above).
  def self.write(serialized, key, value)
    h = {}
    parse(serialized).each do |k, v|
      h[k] = v
    end
    h[key] = value
    @cache_key = nil
    out = "---\n"
    h.each do |k, v|
      out += k.to_s + ": " + (v.nil? ? "" : v.to_s) + "\n"
    end
    out
  end
end
