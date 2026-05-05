module ActionController
  class ParameterMissing < StandardError; end

  # Strong-parameters analogue. Wraps a Hash with the small subset of
  # Rails::ActionController::Parameters real-blog uses: `[]`, `key?`,
  # `fetch`, `to_h`, `merge`, `require(:key)`, `permit(:k1, :k2)`.
  # Keys are normalized to symbols at construction time so callers
  # don't need to remember whether the request body produced strings
  # or symbols.
  class Parameters
    def initialize(hash = {})
      @hash = symbolize_keys(hash)
    end

    # `get` / `set` / `has?` are the primary cross-target API. `[]`
    # / `[]=` / `key?` stay as one-line delegators so Ruby idiom
    # (`params[:title]`, `params.key?(:title)`) keeps working under
    # CRuby, while non-Ruby targets that can't express operator
    # methods (TS, Python, Elixir, Go, Rust) emit `get`/`set`/`has?`
    # directly. A future caller-side lowerer will rewrite Ruby
    # `params[k]` Send nodes to `params.get(k)` for those targets.
    def get(key)
      @hash[key.to_sym]
    end

    def set(key, value)
      @hash[key.to_sym] = value
    end

    def has?(key)
      @hash.key?(key.to_sym)
    end

    def [](key)
      get(key)
    end

    def []=(key, value)
      set(key, value)
    end

    def key?(key)
      has?(key)
    end

    def fetch(key, default = nil)
      sym = key.to_sym
      return @hash[sym] if @hash.key?(sym)
      default
    end

    def empty?
      @hash.empty?
    end

    def to_h
      copy = {}
      @hash.each { |k, v| copy[k] = v }
      copy
    end

    # Accepts a Hash. Callers holding a Parameters call `.to_h` first.
    # Monomorphic param type keeps the slot one-shape for spinel and
    # for type-strict targets (Rust, Crystal, Go).
    def merge(other_hash)
      Parameters.new(@hash.merge(symbolize_keys(other_hash)))
    end

    # `params.require(:article)` — returns the nested Parameters for
    # the given key; raises ParameterMissing when the value is nil,
    # not a Hash, or an empty Hash. Real-blog only requires keys whose
    # values are Hashes (request-body nested data).
    def require(key)
      val = @hash[key.to_sym]
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.nil?
      raise ParameterMissing, "param is missing or the value is empty: #{key}" unless val.is_a?(Hash)
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.empty?
      Parameters.new(val)
    end

    # `params.permit([:title, :body])` — returns a new Parameters
    # with only the listed keys. Takes an Array[Symbol] (no splat) so
    # the parameter shape is monomorphic for spinel + type-strict
    # targets; callers used to writing `params.permit(:title, :body)`
    # in Rails idiom go through the controller lowerer, which emits
    # the Array form.
    def permit(allowed_keys)
      filtered = {}
      allowed_keys.each do |key|
        sym = key.to_sym
        filtered[sym] = @hash[sym] if @hash.key?(sym)
      end
      Parameters.new(filtered)
    end

    # Internal: walk a Hash recursively, symbolizing String keys and
    # recursing into nested Hashes. Values that aren't Hashes pass
    # through unchanged.
    def symbolize_keys(hash)
      out = {}
      hash.each do |k, v|
        sym = k.is_a?(Symbol) ? k : k.to_s.to_sym
        out[sym] = v.is_a?(Hash) ? symbolize_keys(v) : v
      end
      out
    end
  end
end
