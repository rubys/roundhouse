# frozen_string_literal: true
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

  # The store column's reader (`lower::typed_store`'s
  # `push_store_hash_methods`): the gem's TypedHash, a Hash with
  # indifferent access holding every declared attribute (its default
  # until written) and any other stored key. Reads and writes go through
  # the column, so `shop.settings[:plan_name] = "x"` is what the record
  # saves; the record's generated `__rh_<column>_keys`,
  # `__rh_<column>_default(key)` and `__rh_<column>_cast(key, value)`
  # carry the declarations.
  class Accessor
    include Enumerable

    def initialize(record, column)
      @record = record
      @ivar = :"@#{column}"
      @column = column
    end

    def [](key)
      k = key.to_s
      TypedStore.read(serialized, k, @record.__send__(:"__rh_#{@column}_default", k))
    end

    def []=(key, value)
      k = key.to_s
      cast = @record.__send__(:"__rh_#{@column}_cast", k, value)
      @record.instance_variable_set(@ivar, TypedStore.write(serialized, k, cast))
      value
    end

    def key?(key)
      k = key.to_s
      declared.include?(k) || TypedStore.parse(serialized).key?(k)
    end
    alias has_key? key?
    alias include? key?
    alias member? key?

    def fetch(key, *default)
      return self[key] if key?(key)
      return yield(key) if block_given?
      return default.first unless default.empty?
      raise KeyError, "key not found: #{key.to_s.inspect}"
    end

    def dig(key, *rest)
      value = self[key]
      rest.empty? || value.nil? ? value : value.dig(*rest)
    end

    def keys
      (declared + TypedStore.parse(serialized).keys).uniq
    end

    def to_h
      h = defined?(ActiveSupport::HashWithIndifferentAccess) ? ActiveSupport::HashWithIndifferentAccess.new : {}
      keys.each { |k| h[k] = self[k] }
      h
    end
    alias to_hash to_h

    def each(&block)
      return to_enum(:each) unless block
      to_h.each(&block)
      self
    end

    def ==(other)
      to_h == (other.is_a?(Accessor) ? other.to_h : other)
    end

    def inspect
      to_h.inspect
    end

    private

    def serialized
      @record.instance_variable_get(@ivar)
    end

    def declared
      @record.__send__(:"__rh_#{@column}_keys")
    end
  end
end
