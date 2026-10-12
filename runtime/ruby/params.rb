# Narrowing accessors for the recursive request-params tree.
#
# `Roundhouse::ParamValue` is `String | Integer | Float | bool | nil |
# Hash[String, ParamValue] | Array[ParamValue]`, realized per target
# (Crystal alias, TS union, rust `serde_json::Value`, Go `any`). Until
# now that per-target file carried a TYPE AND NO OPERATIONS, so every consumer had to narrow at
# its own call site with `is_a?` — and each emitter recognized that as
# an IDIOM (a type test in `If`-condition position returning the
# narrowed value) rather than modeling the type test itself.
#
# The idiom is fine until you need the same fact somewhere else. Asking
# "was this key provided" three different ways broke three different
# targets, each measured against its toolchain suite:
#
#   `k? && v.is_a?(String)`             go2 — `is_a?(String)` has a
#     type-assertion arm only as an If CONDITION; in a boolean
#     expression it emits `v.IsAPred` against an undefined `String`.
#   `k? && !v.nil?`                     rust2 — renders `nil?` as
#     `is_none()`, which `serde_json::Value` doesn't have (`is_null`).
#   `if v.is_a?(String) then k? else false end`
#                                       go2 + csharp — their narrowing
#     arms assume the branches ARE the narrowed value.
#
# So the operations live here, once, written in the one shape every
# emitter already handles, and callers get ordinary typed calls they can
# use in any position. Same two-layer split as `Db`, except `Db` wraps
# per-target FFI and this is pure logic over the union — so it belongs
# in the transpiled runtime rather than in twelve hand-written copies.
module Params
  # The nested resource hash a controller's params arrive under
  # (`{"article" => {"title" => …}}`). Missing, or present but not a
  # hash, yields an empty hash — a caller reading fields off it then
  # sees them all as not-provided, which is what Rails does with a
  # request that omitted the resource key entirely.
  def self.sub(params, key)
    value = params.fetch(key, {})
    if value.is_a?(Hash)
      value
    else
      {}
    end
  end

  # The scalar under `key`, or `fallback` when the key is absent or
  # holds a non-scalar. Rails' `permit` drops a nested hash or array
  # supplied under a scalar key, so both collapse to the fallback.
  def self.str(sub, key, fallback)
    value = sub.fetch(key, fallback)
    if value.is_a?(String)
      value
    else
      fallback
    end
  end

  # Did the request actually carry a usable value for `key`?
  #
  # Named without a `?` on purpose: predicate mangling for a MODULE
  # function isn't uniform (rust2 makes `provided_pred`, the TS Module
  # mode emits a bare `?` and fails to parse). This is generated code,
  # so portability beats the Ruby idiom.
  #
  # MEASURED against Rails 8.1: `permit` keeps a blank `""` and
  # `Parameters#compact` drops only an explicit nil. So a blank field
  # counts as provided; an absent key, a JSON null, and a non-scalar do
  # not. That distinction is what lets `update` leave a column alone
  # instead of writing `""` over it.
  #
  # The `is_a?` stays in narrowing position (returning the value or
  # nil); the answer is then a plain nil check on a `String?` local,
  # which every target handles — unlike a nil check on the union itself.
  # `params.require(:url)` — Rails' assertion that a key was supplied,
  # answering the value so the caller can go on using it.
  #
  # RAISES on absent-or-blank, which is the whole point of the method:
  # a controller writes it to turn a missing parameter into a 400
  # instead of a nil that fails somewhere later. `Params.sub` is the
  # forgiving twin (missing yields `{}`) and is what the permit chain
  # uses; the two are not interchangeable, and campfire's
  # `unfurl_links_controller` asserts precisely this raise.
  #
  # A free function over the Hash rather than a method on it: `@params`
  # IS a plain Hash here, and `require` on one reaches Kernel's private
  # method — "private method 'require' called for an instance of Hash",
  # which is how this gap announced itself in four tests.
  #
  # "Blank" is Rails' `require` test, `value.present? || value == false`:
  # nil, a whitespace-only string, and an EMPTY hash or array raise too.
  # The empty hash is the one that matters: ParamsWrapper wraps a JSON
  # body with nothing to wrap (an empty body, `{}`, only keys the model
  # lacks) as `key => {}`, and Rails 8.1 answers that with 400 rather
  # than letting the action run on no parameters.
  def self.require_key(params, key)
    unless params.key?(key)
      raise(ActionController::ParameterMissing.new(key))
    end
    value = params.fetch(key, "")
    if value != false && ActiveSupport.blank?(value)
      raise(ActionController::ParameterMissing.new(key))
    end
    value
  end

  # Rails' ParamsWrapper, the per-request half (the compiler decided the
  # rest: whether this controller wraps, the key, the keys copied). When
  # the request's body is JSON and `params` has no `key` yet, the BODY
  # params - never the query string or the path - are copied under
  # `key`: only `include` when `use_include`, else every key but
  # `exclude` and Rails' own `authenticity_token _method utf8`. The
  # top-level keys stay where they are, as in Rails.
  def self.wrap(params, request, key, use_include, include, exclude)
    # A controller driven without a request (a test calling
    # `process_action` directly) has no body to wrap.
    return params if request.nil?
    return params if params.key?(key)
    content_type = request.env.fetch("CONTENT_TYPE", "").to_s
    return params if content_type.empty?
    begin
      return params unless Mime::Type.lookup(content_type).to_sym == :json
    rescue Mime::Type::InvalidMimeType
      return params
    end
    body = request.request_parameters
    wrapped = {}
    if use_include
      i = 0
      while i < include.length
        name = include[i]
        wrapped[name] = body.fetch(name, "") if body.key?(name)
        i = i + 1
      end
    else
      body.each do |name, value|
        next if exclude.include?(name)
        next if name == "authenticity_token" || name == "_method" || name == "utf8"
        wrapped[name] = value
      end
    end
    params[key] = wrapped
    params
  end

  # `params.expect(key => [fields])`'s refusal, answered before the
  # typed factory reads the resource: `params` back unchanged, or
  # `ParameterMissing` (400 when the app does not rescue it). Rails
  # refuses unless the value is a Hash holding at least one permitted
  # key with the kind of value its filter takes: a scalar under a scalar
  # key (`:title`), an array of scalars under `tags: []`, a hash under
  # `settings: [:theme]` or `settings: {}`, an array under
  # `items: [[:name]]`. A missing key, nil, "", {}, a scalar, an array,
  # and a hash of only unpermitted or mistyped keys all raise.
  def self.expect_present(params, key, fields, scalar_array_fields, hash_fields, array_fields)
    value = params.fetch(key, "")
    if value.is_a?(Hash)
      i = 0
      while i < fields.length
        field = fields[i]
        if value.key?(field)
          inner = value.fetch(field, "")
          return params unless inner.is_a?(Hash) || inner.is_a?(Array)
        end
        i = i + 1
      end
      i = 0
      while i < scalar_array_fields.length
        field = scalar_array_fields[i]
        if value.key?(field)
          inner = value.fetch(field, "")
          return params if inner.is_a?(Array) && Params.scalars_only(inner)
        end
        i = i + 1
      end
      i = 0
      while i < hash_fields.length
        field = hash_fields[i]
        return params if value.key?(field) && value.fetch(field, "").is_a?(Hash)
        i = i + 1
      end
      i = 0
      while i < array_fields.length
        field = array_fields[i]
        return params if value.key?(field) && value.fetch(field, "").is_a?(Array)
        i = i + 1
      end
    end
    raise(ActionController::ParameterMissing.new(key))
  end

  # Does `items` hold only scalars - what `tags: []` permits?
  def self.scalars_only(items)
    i = 0
    while i < items.length
      item = items[i]
      return false if item.is_a?(Hash) || item.is_a?(Array)
      i = i + 1
    end
    true
  end

  # `params.require(key).permit(...)`'s refusal. Laxer than `expect`, as
  # in Rails, whose `require` passes on a value that is `present?` or
  # `false`: a blank one (missing, nil, a whitespace-only string, an empty
  # array or hash) raises `ParameterMissing`; a hash of only unpermitted
  # keys passes (the permit then yields nothing); any other value reaches
  # `permit`, which only a hash has, so the request is a 500 there and here.
  def self.require_present(params, key)
    value = params.fetch(key, "")
    if value.is_a?(Hash)
      raise(ActionController::ParameterMissing.new(key)) if value.empty?
      return params
    end
    if value != false && ActiveSupport.blank?(value)
      raise(ActionController::ParameterMissing.new(key))
    end
    raise(NoMethodError.new("undefined method 'permit' for #{key}: not a Hash"))
  end

  # One key of a top-level `params.permit(:a, :b)` — the Rails 8
  # authentication generator's `@user.update(params.permit(:password,
  # :password_confirmation))`. The permit lowers to a chain of these from
  # `{}`, one per permitted key: `name` (the Symbol the model's
  # `update`/`create` read the field under) is added when the request
  # carried a scalar for `key`, and left out otherwise — Rails' `permit`
  # omits an absent key and drops a non-scalar, and a model write then
  # leaves that attribute alone.
  def self.permitted(acc, params, key, name)
    acc[name] = Params.str(params, key, "") if Params.provided(params, key)
    acc
  end

  def self.provided(sub, key)
    return false unless sub.key?(key)
    value = sub.fetch(key, "")
    narrowed = if value.is_a?(String)
      value
    else
      nil
    end
    !narrowed.nil?
  end
end
