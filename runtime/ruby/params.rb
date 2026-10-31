# frozen_string_literal: true
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
  def self.expect_array(params, key, fields, scalar_array_fields)
    children = {}
    modes = {}
    scalar_array_fields.each do |field|
      children[field] = Params.no_fields
      modes[field] = "scalars"
    end
    out = []
    value = params.fetch(key, nil)
    if value.is_a?(Array)
      value.each do |item|
        out.push(Params.filtered_values(item, fields, children, modes)) if item.is_a?(Hash)
      end
    end
    raise(ActionController::ParameterMissing.new(key)) if out.empty?
    out
  end

  def self.filtered_values(sub, fields, children, modes)
    filtered = Params.allowed_scalars(sub, fields)
    children.each do |key, allowed|
      mode = modes.fetch(key, "hash")
      if mode == "scalars"
        filtered[key] = sub.fetch(key, nil) if Params.scalar_list_provided(sub, key)
      else
        value = Params.nested(sub, key, allowed, mode)
        filtered[key] = value unless value.nil?
      end
    end
    filtered
  end

  def self.record_list(sub, key, allowed)
    value = Params.nested(sub, key, allowed, "list")
    if value.is_a?(Array)
      records = []
      value.each do |item|
        records.push(item) if item.is_a?(Hash)
      end
      records
    elsif value.is_a?(Hash)
      numbered = {}
      value.each do |name, item|
        numbered[name] = item if item.is_a?(Hash)
      end
      numbered
    else
      nil
    end
  end

  def self.nested_key(key)
    if key.is_a?(Symbol)
      key.to_s
    else
      key
    end
  end

  def self.nested_lookup(value, key)
    if value.is_a?(Hash)
      value.fetch(Params.nested_key(key), nil)
    elsif value.is_a?(Array)
      value[key]
    else
      raise(NoMethodError.new("the nested params value is not a Hash"))
    end
  end

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
      # A fresh string with the same characters. The hash may still
      # hold a handle another name appends to; this method only answers
      # the scalar for reading.
      value + ""
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
        # `permit` keeps a scalar's multiparameter keys (`title(1)`) too.
        return params if Params.multiparameter_scalar_given(value, field)
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

  # A non-scalar permitted key's value, filtered as Rails' `permit`
  # filters it, or nil when nothing under `key` passes. `allowed` is the
  # key's flat list of scalar keys; `mode` is how it was spelled:
  #   "list"  `k: [[:a]]`: an Array of hashes, or a hash of hashes
  #           (`{"0" => {...}}`, what form builders send);
  #   "any"   `permit(k: [:a])`: those, or one hash;
  #   "hash"  `expect(r: [k: [:a]])`: one hash only;
  #   "deep"  `k: {}`: any hash, as submitted.
  # Each kept hash keeps the allowed keys holding a scalar.
  def self.nested(sub, key, allowed, mode)
    return nil unless sub.key?(key)
    value = sub.fetch(key, nil)
    if value.is_a?(Array)
      return nil if mode == "hash" || mode == "deep"
      kept = []
      i = 0
      while i < value.length
        item = value[i]
        kept.push(Params.allowed_scalars(item, allowed)) if item.is_a?(Hash)
        i = i + 1
      end
      return kept
    end
    return nil unless value.is_a?(Hash)
    return value if mode == "deep"
    if mode != "hash" && Params.numbered(value)
      out = {}
      value.each do |k, v|
        out[k] = Params.allowed_scalars(v, allowed) if Params.index_key(k) && v.is_a?(Hash)
      end
      return out
    end
    return nil if mode == "list"
    Params.allowed_scalars(value, allowed)
  end

  # `k: []`'s value: the Array of scalars under `key`, each as the String
  # a request carries, or [] when the key is absent or holds anything
  # else (a scalar, a hash, an array with a hash or array in it), which
  # Rails' `permit` drops. `scalar_list_provided` tells the two apart.
  def self.scalar_list(sub, key)
    out = []
    return out unless Params.scalar_list_provided(sub, key)
    value = sub.fetch(key, nil)
    if value.is_a?(Array)
      i = 0
      while i < value.length
        item = value[i]
        out.push(item.nil? ? "" : item.to_s)
        i = i + 1
      end
    end
    out
  end

  def self.scalar_list_provided(sub, key)
    return false unless sub.key?(key)
    value = sub.fetch(key, nil)
    value.is_a?(Array) && Params.scalars_only(value)
  end

  def self.nested_provided(sub, key, allowed, mode)
    !Params.nested(sub, key, allowed, mode).nil?
  end

  # Permitted scalar keys, including matching multiparameter keys.
  def self.allowed_scalars(hash, allowed)
    out = {}
    i = 0
    while i < allowed.length
      name = allowed[i]
      if hash.key?(name)
        item = hash.fetch(name, nil)
        out[name] = item unless item.is_a?(Hash) || item.is_a?(Array)
      end
      Params.multiparameter_names(hash, name).each do |part|
        item = hash.fetch(part, nil)
        out[part] = item unless item.is_a?(Hash) || item.is_a?(Array)
      end
      i = i + 1
    end
    out
  end

  # Rails' `nested_attributes?`: some key is an index ("0", "-1")
  # holding a hash.
  def self.numbered(hash)
    found = false
    hash.each do |k, v|
      found = true if Params.index_key(k) && v.is_a?(Hash)
    end
    found
  end

  def self.index_key(key)
    text = key.to_s
    text = text[1, text.length - 1] if text.start_with?("-")
    return false if text.empty?
    i = 0
    while i < text.length
      return false unless "0123456789".include?(text[i])
      i = i + 1
    end
    true
  end

  # A permitted scalar as the generated `from_raw` reads it, with its
  # MULTIPARAMETER keys. Rails' `permit(:k)` keeps `k(1)`, `k(2i)`,
  # `k(3f)` — the parts `date_select` submits — and assignment rebuilds
  # `k` from them: a Date for a date column, and for a String or Text
  # attribute the text of the parts Hash (`name(1)=x` stores
  # `{1 => "x"}`, Ruby 3.4's Hash#inspect). That text is reproduced here
  # (`multiparameter_cast`) for parts that are plain text or `i`
  # integers, so validations see and a save writes what Rails' would;
  # the model refuses (raises) for any other column type, and every
  # other shape is `multiparameter_unsupported`.
  def self.scalar_provided(sub, key)
    Params.provided(sub, key) || Params.multiparameter_cast(sub, key) != ""
  end

  # The multiparameter value wins, as Rails assigns those keys last.
  def self.scalar(sub, key)
    cast = Params.multiparameter_cast(sub, key)
    return cast if cast != ""
    Params.str(sub, key, "")
  end

  # An empty field list, typed: what a params object holds before
  # `from_raw` reads a request.
  def self.no_fields
    out = [""]
    out.pop
    out
  end

  # Whether the request holds a scalar under one of `key`'s
  # multiparameter keys — what `permit(:key)` keeps.
  def self.multiparameter_scalar_given(sub, key)
    given = false
    Params.multiparameter_names(sub, key).each do |name|
      inner = sub.fetch(name, "")
      given = true unless inner.is_a?(Hash) || inner.is_a?(Array)
    end
    given
  end

  # Whether `list` holds any of `names`.
  def self.any_of(list, names)
    found = false
    names.each do |name|
      found = true if list.include?(name)
    end
    found
  end

  # The permitted `keys` the request sent multiparameter keys for.
  def self.multiparameter_fields(sub, keys)
    out = [""]
    out.pop
    keys.each do |key|
      out << key if Params.multiparameter_names(sub, key).length > 0
    end
    out
  end

  # Whether any of `keys` has multiparameter keys whose value Rails
  # builds in a way not reproduced: a non-String part, an `f` part, a
  # part whose text Hash#inspect would escape, or only blank parts
  # (Rails then assigns nil).
  def self.multiparameter_unsupported(sub, keys)
    unsupported = false
    keys.each do |key|
      names = Params.multiparameter_names(sub, key)
      unsupported = true if names.length > 0 && Params.multiparameter_cast(sub, key) == ""
    end
    unsupported
  end

  # Rails' value for `key` from its multiparameter keys, as the text a
  # String attribute casts it to — `{1 => "x", 2 => 7, 3 => nil}`, in
  # first-seen position order, a repeated position keeping its first
  # value — or "" when there are none or the shape is unsupported.
  def self.multiparameter_cast(sub, key)
    names = Params.multiparameter_names(sub, key)
    return "" if names.length == 0
    positions = [""]
    positions.pop
    rendered = [""]
    rendered.pop
    any_value = false
    ok = true
    names.each do |name|
      inner = name[key.length + 1, name.length - key.length - 2]
      suffix = inner[inner.length - 1, 1]
      digits = inner
      digits = inner[0, inner.length - 1] if suffix == "i" || suffix == "f"
      ok = false if suffix == "f"
      ok = false unless Params.provided(sub, name)
      text = Params.str(sub, name, "")
      part = "nil"
      if text != ""
        any_value = true
        if suffix == "i"
          ok = false unless Params.plain_integer(text)
          part = text.to_i.to_s
        else
          ok = false unless Params.plain_text(text)
          part = "\"" + text + "\""
        end
      end
      unless positions.include?(digits)
        positions << digits
        rendered << digits + " => " + part
      end
    end
    return "" unless ok && any_value
    "{" + rendered.join(", ") + "}"
  end

  # The request's multiparameter keys for `key`: `key(N)`, `key(Ni)`,
  # `key(Nf)` — Rails' `/\(\d+[if]?\)\z/` after `key` — in request
  # order. N is read back through `to_i`, so a zero-padded position
  # (`key(01)`) is not one.
  def self.multiparameter_names(sub, key)
    out = [""]
    out.pop
    sub.keys.each do |name|
      out << name if Params.multiparameter_key(name, key)
    end
    out
  end

  def self.multiparameter_key(name, key)
    return false unless name.length > key.length + 2
    return false unless name.start_with?(key + "(") && name.end_with?(")")
    position = name[key.length + 1, name.length - key.length - 2]
    if position.end_with?("i") || position.end_with?("f")
      position = position[0, position.length - 1]
    end
    position.length > 0 && position.to_i.to_s == position
  end

  # Text Hash#inspect writes between quotes unchanged: printable ASCII
  # other than `"`, `\\` and `#`.
  def self.plain_text(text)
    i = 0
    n = text.bytesize
    plain = true
    while i < n
      b = text.getbyte(i)
      plain = false if b < 32 || b > 126 || b == 34 || b == 92 || b == 35
      i += 1
    end
    plain
  end

  # An optional `-` and digits, the text whose `to_i` is the number it
  # spells.
  def self.plain_integer(text)
    digits = text
    digits = text[1, text.length - 1] if text.start_with?("-")
    digits.length > 0 && digits.to_i.to_s == digits
  end
end
