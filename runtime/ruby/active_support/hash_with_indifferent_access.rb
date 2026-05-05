module ActiveSupport
  # Hash that stores keys as Strings internally and accepts either
  # Symbol or String at the API surface — Rails' canonical solution
  # for the params/session/flash/content_for store types where
  # callers reach in with both forms (`params[:id]` or `params["id"]`).
  #
  # Storing keys as Strings is portable across every transpile target:
  # Ruby's `String#to_sym` is per-target (Crystal/Elixir forbid
  # dynamic Symbol/atom creation), but `Symbol#to_s` is universal.
  # Defining HWIA as String-internal lets every long-lived runtime
  # state hash (Parameters, session, flash, ViewHelpers slots) cross
  # all targets without per-emitter Symbol→String rewrites.
  #
  # Recursive normalization: nested `Hash` values become nested HWIA
  # instances on insert, so `params[:user][:name]` walks a chain of
  # HWIAs uniformly. Other value types (String, Integer, Array, the
  # transpiled framework's other classes) pass through unchanged.
  class HashWithIndifferentAccess
    def initialize(other = nil)
      @data = {}
      if !other.nil?
        other.each do |k, v|
          @data[k.to_s] = normalize_value(v)
        end
      end
    end

    def [](key)
      @data[key.to_s]
    end

    def []=(key, value)
      @data[key.to_s] = normalize_value(value)
    end

    def key?(key)
      @data.key?(key.to_s)
    end

    def has_key?(key)
      @data.key?(key.to_s)
    end

    def include?(key)
      @data.key?(key.to_s)
    end

    def fetch(key, default = nil)
      k = key.to_s
      return @data[k] if @data.key?(k)
      default
    end

    def delete(key)
      @data.delete(key.to_s)
    end

    # Returns a new HWIA with `other`'s entries merged on top. `other`
    # may be a Hash or another HWIA — either responds to `each` with
    # `(k, v)` pairs, and the receiving HWIA normalizes the keys.
    def merge(other)
      result = HashWithIndifferentAccess.new(self)
      other.each do |k, v|
        result[k] = v
      end
      result
    end

    # Returns the underlying String-keyed Hash. By Rails convention
    # `.to_h` on HWIA preserves the String-keyed shape (it's the
    # "indifferent" half that goes away — the underlying storage is
    # always String-keyed).
    def to_h
      @data
    end

    def empty?
      @data.empty?
    end

    def length
      @data.length
    end

    def size
      @data.length
    end

    def keys
      @data.keys
    end

    def values
      @data.values
    end

    def each
      @data.each do |k, v|
        yield k, v
      end
      self
    end

    # Internal: normalize a value on insert. Plain Hashes recursively
    # become HWIAs so deep access (`params[:user][:name]`) walks a
    # uniform chain. HWIA instances pass through unchanged (already
    # normalized). Other types (String, Integer, Array, framework
    # classes) pass through.
    def normalize_value(value)
      if value.is_a?(HashWithIndifferentAccess)
        value
      elsif value.is_a?(Hash)
        HashWithIndifferentAccess.new(value)
      else
        value
      end
    end
  end
end
