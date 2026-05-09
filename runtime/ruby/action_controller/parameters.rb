module ActionController
  class ParameterMissing < StandardError; end

  # Strong-parameters analogue with indifferent access (Symbol or
  # String key). Internal storage is a String-keyed Hash whose
  # values are either flat `String` (form fields, query string,
  # path captures) or nested `Parameters` (resource-shaped bodies
  # like `{article: {title: ...}}`).
  #
  # Earlier revisions wrapped `ActiveSupport::HashWithIndifferentAccess`,
  # but HWIA's `untyped` value channel collapsed to `String` at the
  # Crystal emit boundary and forced every consumer through a
  # heterogeneous-Hash channel that strict-typed targets couldn't
  # commit to. The recursive `Parameters` storage gives strict
  # targets a concrete value union (`String | Parameters`) without
  # losing the nested-resource shape.
  class Parameters
    def initialize(hash = nil)
      @hash = {}
      return if hash.nil?
      # Index-walk over `.keys` (cross-target idiom — TS plain
      # objects don't have `each_pair` / `each`). Each value flows
      # through `normalize` which produces a uniformly-typed
      # `String | Parameters` regardless of the input's value-type
      # union, so the Crystal emit's ivar inference doesn't pick up
      # transient Nils from the input's nilable lookup.
      keys = hash.keys
      i = 0
      while i < keys.length
        k = keys[i]
        @hash[k.to_s] = normalize(hash[k])
        i += 1
      end
    end

    # Normalize a raw value into the typed-storage union: Hash → nested
    # Parameters; everything else passes through to a String (callers
    # supply String / Symbol / Integer values; framework code ultimately
    # treats them as Strings via `.to_s` at access points).
    def normalize(value)
      return Parameters.new(value) if value.is_a?(Hash)
      value.to_s
    end

    # `get` / `set` / `has?` are the cross-target named-method API.
    # `[]` / `[]=` / `key?` stay as Ruby idiom; targets see the named
    # forms after the lowerer's bracket→`.get` rewrite for typed
    # receivers (see `controller_to_library/rewrites.rs`).
    def get(key)
      @hash[key.to_s]
    end

    def set(key, value)
      @hash[key.to_s] = normalize(value)
    end

    def has?(key)
      @hash.key?(key.to_s)
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
      k = key.to_s
      return @hash[k] if @hash.key?(k)
      default
    end

    def empty?
      @hash.empty?
    end

    # `to_h` exposes the inner String-keyed Hash with nested
    # Parameters values left as-is. Callers that need flat
    # nested-Hash output chain `.to_h` per level — matches the
    # earlier HWIA-backed contract and avoids the per-target
    # Hash-value-type widening that recursive unwrap would force
    # (the result type would be `String | Hash | Nil`, which
    # collapses ergonomically on strict targets).
    def to_h
      @hash
    end

    def merge(other_hash)
      Parameters.new(to_h.merge(other_hash))
    end

    # `params.require(:article)` — returns the nested Parameters.
    # Raises ParameterMissing when the value is nil, not Parameters
    # (i.e. a flat scalar), or an empty Parameters.
    #
    # `@hash.fetch(k, nil)` instead of `@hash[k]` so Crystal's
    # strict `Hash#[]` (which raises KeyError on missing) doesn't
    # short-circuit the ParameterMissing path. CRuby's `Hash#[]`
    # returns nil; same effect.
    def require(key)
      raw = @hash.fetch(key.to_s, nil)
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if raw.nil?
      val = raw.is_a?(Parameters) ? raw : nil
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.nil?
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.empty?
      val
    end

    # `params.permit([:title, :body])` — returns filtered Parameters.
    # Builds the result via `set` rather than an intermediate Hash
    # so the typed value union (`String | Parameters`) flows
    # cleanly. An intermediate `filtered = {}` infers as
    # `Hash(String, String)` on strict targets and rejects the
    # nested-Parameters value side.
    def permit(allowed_keys)
      result = Parameters.new
      i = 0
      while i < allowed_keys.length
        k = allowed_keys[i].to_s
        result.set(k, @hash[k]) if @hash.key?(k)
        i += 1
      end
      result
    end
  end
end
