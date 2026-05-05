require_relative "../active_support/hash_with_indifferent_access"

module ActionController
  class ParameterMissing < StandardError; end

  # Strong-parameters analogue. Wraps an
  # ActiveSupport::HashWithIndifferentAccess so callers can use either
  # `params[:title]` or `params["title"]` interchangeably (the legacy
  # behavior `Hash[Symbol]` keys provided via `.to_sym` normalization,
  # but Crystal/Elixir forbid dynamic Symbol creation; HWIA stores
  # everything as String internally and normalizes input via
  # `Symbol#to_s`, which is universal across targets).
  class Parameters
    def initialize(hash = nil)
      @hash = ActiveSupport::HashWithIndifferentAccess.new(hash)
    end

    # `get` / `set` / `has?` are the cross-target named-method API.
    # `[]` / `[]=` / `key?` are one-line delegators kept for Ruby
    # idiom under CRuby; non-Ruby emits use the named forms.
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

    # Returns a plain String-keyed Hash. Per Rails HWIA convention,
    # `to_h` exposes the underlying String-keyed storage (the
    # "indifferent" half goes away).
    def to_h
      @hash.to_h
    end

    # Accepts a Hash or HWIA. The HWIA constructor normalizes either
    # form on the way in.
    def merge(other_hash)
      Parameters.new(@hash.merge(other_hash))
    end

    # `params.require(:article)` — returns the nested Parameters for
    # the given key; raises ParameterMissing when the value is nil,
    # not a Hash, or an empty Hash. The HWIA recursive-normalize means
    # nested Hash values are stored as nested HWIAs already, so the
    # is_a? check accepts either form.
    def require(key)
      val = @hash[key]
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.nil?
      if !val.is_a?(ActiveSupport::HashWithIndifferentAccess) && !val.is_a?(Hash)
        raise ParameterMissing, "param is missing or the value is empty: #{key}"
      end
      raise ParameterMissing, "param is missing or the value is empty: #{key}" if val.empty?
      Parameters.new(val)
    end

    # `params.permit([:title, :body])` — returns a new Parameters with
    # only the listed keys. Takes an Array (Symbol or String elements;
    # HWIA normalizes either) so the parameter shape is monomorphic
    # for type-strict targets; the controller lowerer emits the Array
    # form from Rails idiom `params.expect(article: [:title, :body])`.
    def permit(allowed_keys)
      filtered = ActiveSupport::HashWithIndifferentAccess.new
      allowed_keys.each do |key|
        filtered[key] = @hash[key] if @hash.key?(key)
      end
      Parameters.new(filtered)
    end
  end
end
