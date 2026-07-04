# typed_store gem subset (CRuby overlay) — YAML-serialized virtual
# attributes backed by a TEXT column. Lobsters' User declares
# `typed_store :settings do |s| s.string :totp_secret … end`; the Ruby
# emit path (`apply_typed_store_lowering`) synthesizes per-attribute
# readers/writers that route through this module, so the model file
# stays declarative and the serialization format lives in one place.
#
# The store column holds a YAML hash with string keys
# (`prefers_color_scheme: system\n…` in the benchmark DB). Reads far
# outnumber writes (the benchmark never writes settings), so `parse`
# keeps a one-entry cache keyed by the serialized string — /settings
# reads ~24 attributes off the same row without re-parsing per
# attribute.
require "yaml"

module TypedStore
  @cache_key = nil
  @cache_val = {}

  def self.parse(serialized)
    s = serialized.to_s
    return @cache_val if s == @cache_key
    h = s.empty? ? {} : YAML.safe_load(s)
    h = {} unless h.is_a?(Hash)
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
  # assigns it back to the column ivar.
  def self.write(serialized, key, value)
    h = parse(serialized).dup
    h[key] = value
    @cache_key = nil
    YAML.dump(h)
  end
end
