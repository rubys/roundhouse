module ActionDispatch
  # Per-app session store. Real-blog uses no session keys, so the
  # struct is empty (no typed fields). It still exposes HWIA-shape
  # shim methods so framework tests calling `@controller.session.length()`
  # compile and pass. Apps with session schema grow this struct in
  # parallel with their controller usage (the typed-targets pipeline
  # picks up new fields via the same scan that drives Flash).
  #
  # Internal `@data` Hash is kept as the storage so future per-app
  # session keys can be threaded through without a runtime rewrite —
  # the shim methods already route through it.
  class Session
    def initialize(other = nil)
      @data = {}
      return if other.nil?
      keys = other.keys
      i = 0
      while i < keys.length
        k = keys[i]
        v = other[k]
        @data[k.to_s] = v
        i += 1
      end
    end

    def [](key)
      k = key.to_s
      return @data[k] if @data.key?(k)
      nil
    end

    def []=(key, value)
      @data[key.to_s] = value
      value
    end

    def fetch(key, default = nil)
      k = key.to_s
      return @data[k] if @data.key?(k)
      default
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

    def delete(key)
      @data.delete(key.to_s)
    end

    def length
      @data.length
    end

    def size
      @data.length
    end

    def empty?
      @data.empty?
    end

    def keys
      @data.keys
    end

    def values
      @data.values
    end

    def each
      keys = @data.keys
      i = 0
      while i < keys.length
        k = keys[i]
        v = @data[k]
        yield k, v
        i += 1
      end
      self
    end

    def to_h
      @data
    end

    def merge(other)
      result = Session.new(to_h)
      other.each do |k, v|
        result[k] = v
      end
      result
    end
  end
end
