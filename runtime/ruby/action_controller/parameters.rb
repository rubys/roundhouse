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
      @hash.each { |k, v| copy[k] = v.is_a?(Parameters) ? v.to_h : v }
      copy
    end

    def merge(other)
      other_hash = other.is_a?(Parameters) ? other.to_h : other
      Parameters.new(@hash.merge(symbolize_keys(other_hash)))
    end

    # `params.require(:article)` — returns the nested Parameters for
    # the given key; raises ParameterMissing when the value is nil or
    # an empty hash.
    def require(key)
      val = @hash[key.to_sym]
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.nil?
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.is_a?(Hash) && val.empty?
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.is_a?(Parameters) && val.empty?
      val.is_a?(Parameters) ? val : (val.is_a?(Hash) ? Parameters.new(val) : val)
    end

    # `params.permit(:title, :body)` — returns a new Parameters with
    # only the listed keys.
    def permit(*allowed)
      filtered = {}
      allowed.each do |key|
        sym = key.to_sym
        filtered[sym] = @hash[sym] if @hash.key?(sym)
      end
      Parameters.new(filtered)
    end

    def symbolize_keys(input)
      return input.to_h if input.is_a?(Parameters)
      out = {}
      input.each do |k, v|
        sym = k.is_a?(Symbol) ? k : k.to_s.to_sym
        out[sym] = if v.is_a?(Hash)
                     symbolize_keys(v)
                   elsif v.is_a?(Parameters)
                     v.to_h
                   else
                     v
                   end
      end
      out
    end
  end
end
