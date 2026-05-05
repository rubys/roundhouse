require_relative "../active_support/hash_with_indifferent_access"

module ActionController
  class ParameterMissing < StandardError; end

  # Strong-parameters analogue. Wraps an
  # ActiveSupport::HashWithIndifferentAccess so callers can use either
  # `params[:title]` or `params["title"]` interchangeably.
  #
  # The HWIA stores keys as Strings and normalizes via `Symbol#to_s`,
  # which is universal across targets (Crystal/Elixir forbid the
  # reverse `String#to_sym`).
  class Parameters
    def initialize(hash = nil)
      @hash = ActiveSupport::HashWithIndifferentAccess.new(hash)
    end

    # `get` / `set` / `has?` are the cross-target named-method API.
    # `[]` / `[]=` / `key?` stay as one-line delegators so Ruby idiom
    # keeps working under CRuby; the TS emit rewrites bracket access
    # on a Parameters instance to `.get`/`.set` automatically.
    def get(key)
      @hash[key]
    end

    def set(key, value)
      @hash[key] = value
    end

    def has?(key)
      @hash.key?(key)
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
      @hash.fetch(key, default)
    end

    def empty?
      @hash.empty?
    end

    # Underlying String-keyed Hash (HWIA convention: `to_h` exposes
    # the inner storage; the "indifferent" half goes away here).
    def to_h
      @hash.to_h
    end

    # Accepts a plain Hash. The merged HWIA is unwrapped via `.to_h`
    # before constructing — HWIA's initialize is typed to take Hash.
    def merge(other_hash)
      Parameters.new(@hash.merge(other_hash).to_h)
    end

    # `params.require(:article)` — returns the nested Parameters.
    # Raises ParameterMissing when the value is nil, not a hash, or
    # an empty hash. HWIA recursively normalizes nested Hash values
    # into nested HWIA on insert, so the value here is HWIA when
    # present.
    def require(key)
      raw = @hash[key]
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if raw.nil?
      # Bind through an `is_a?` ternary so the narrowed type carries
      # to the next line — the body-typer (and the TS emit's class-
      # recv dispatch) sees `val` as HWIA, not the wider Untyped from
      # @hash[key]. Without this binding, `val.empty?` emits as a
      # property reference (always truthy) under the TS no-paren-on-
      # zero-arg fallback for non-class receivers.
      val = raw.is_a?(ActiveSupport::HashWithIndifferentAccess) ? raw : nil
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.nil?
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.empty?
      Parameters.new(val.to_h)
    end

    # `params.permit([:title, :body])` — returns filtered Parameters.
    def permit(allowed_keys)
      filtered = {}
      allowed_keys.each do |key|
        filtered[key.to_s] = @hash[key] if @hash.key?(key)
      end
      Parameters.new(filtered)
    end
  end
end
