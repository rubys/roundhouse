# Rails' request-parameter builder, for the spinel lane: a query string
# or urlencoded body into the nested params a controller reads.
#
# A port of actionpack's `ActionDispatch::QueryParser.each_pair` (split
# and decode) and `ActionDispatch::ParamBuilder#store_nested_param`
# (nest), at the revision campfire pins. The ruby family runs Rack and
# never reaches this file.
#
# WHY A PORT, and not the flat parse it replaces. tep parsed a body into
# a String->String hash, so a repeated key kept its last value, and
# `Main.nest_params` knew one level of one resource. `user_ids[]=2&
# user_ids[]=3` reached campfire's DirectsController as `{"" => "3"}`,
# and every direct room on the binary was created with its creator
# alone while the request answered 302. Rails' own rules are the
# contract: arrays (`a[]`), hashes to any depth (`a[b][c]`), hashes in
# arrays (`a[][b]`), several resources in one request, a key with no
# `=` as nil.
#
# THE ORACLE is Rails itself: tests/fixtures/params_vectors.json holds
# 2,755 query strings and what `ParamBuilder.from_query_string` makes of
# them (null where Rails raises, which it answers with a 400), generated
# by tests/fixtures/params_vectors.rb — borrowed from once-campfire-rust,
# and byte-identical when regenerated with our oracle's bundle.
# tests/spinel_param_builder.rs holds this file to it under CRuby and
# compiled by spinel.
#
# ERRORS are a nil return, not a raise: an invalid %-escape, invalid
# UTF-8 in a key (or a top-level value), a nesting deeper than 100, or a
# name used as both an Array and a Hash. The dispatcher answers 400.
module ParamBuilder
  DEPTH_LIMIT = 100

  # Raised inside a build and rescued by `build`; never escapes.
  class Invalid < StandardError
  end

  # `each_pair`: split on `&` (and the spaces after it), `k=v` at the
  # first `=`, both halves www-form-decoded. A part with no `=` has a nil
  # value, kept apart in `present` so the arrays stay String-typed.
  # Appends to the three arrays; false when a %-escape is malformed.
  def self.split(s, keys, values, present)
    parts = s.split("&")
    i = 0
    while i < parts.length
      part = parts[i]
      if i > 0
        j = 0
        j += 1 while j < part.length && part[j] == " "
        part = part[j, part.length - j].to_s
      end
      if part.length > 0
        eq = part.index("=")
        raw_k = eq.nil? ? part : part[0, eq].to_s
        k = ParamBuilder.decode(raw_k)
        return false if k.nil?
        if eq.nil?
          keys.push(k)
          values.push("")
          present.push(false)
        else
          v = ParamBuilder.decode(part[eq + 1, part.length - eq - 1].to_s)
          return false if v.nil?
          keys.push(k)
          values.push(v)
          present.push(true)
        end
      end
      i += 1
    end
    true
  end

  # `URI.decode_www_form_component`: `+` is a space, `%XX` a byte, and a
  # `%` not followed by two hex digits is an error (nil), as Ruby raises.
  def self.decode(s)
    out = +""
    i = 0
    n = s.length
    while i < n
      c = s[i]
      if c == "+"
        out << " "
        i += 1
      elsif c == "%"
        return nil if i + 2 >= n
        hi = ParamBuilder.hex(s[i + 1].to_s)
        lo = ParamBuilder.hex(s[i + 2].to_s)
        return nil if hi < 0 || lo < 0
        out << (hi * 16 + lo).chr
        i += 3
      else
        out << c.to_s
        i += 1
      end
    end
    out.force_encoding("UTF-8")
  end

  def self.hex(c)
    return c.ord - 48 if c >= "0" && c <= "9"
    return c.ord - 87 if c >= "a" && c <= "f"
    return c.ord - 55 if c >= "A" && c <= "F"
    -1
  end

  # The nested params for the pairs, or nil where Rails answers 400.
  def self.build(keys, values, present)
    params = {}
    i = 0
    while i < keys.length
      v = present[i] ? values[i] : nil
      ParamBuilder.store(params, keys[i], v, 0)
      i += 1
    end
    params
  rescue ParamBuilder::Invalid
    nil
  end

  # A query string straight to params (nil on a 400).
  def self.from_query_string(s)
    keys = []
    values = []
    present = []
    return nil unless ParamBuilder.split(s, keys, values, present)
    ParamBuilder.build(keys, values, present)
  end

  # A JSON request body already has nested Hash/Array structure, so it
  # does not go through the bracket-name parser above. Copy its root and
  # recursively normalize the JSON tree like Rails'
  # ActionDispatch::ParamBuilder#from_hash. With Rails' default
  # `perform_deep_munge`, nil array members are removed at every depth;
  # nil Hash values and empty containers are retained.
  def self.from_json(params)
    out = {}
    params.each do |k, v|
      out[k.to_s] = ParamBuilder.normalize_json(v)
    end
    out
  end

  # Convert a JSON subtree to Rails' parameter shape: stringify object
  # keys recursively, drop nil array members, and retain nil object
  # values, scalar leaves, and empty containers.
  def self.normalize_json(value)
    if value.is_a?(Hash)
      out = {}
      value.each do |k, v|
        out[k.to_s] = ParamBuilder.normalize_json(v)
      end
      out
    elsif value.is_a?(Array)
      out = []
      value.each do |v|
        out.push(ParamBuilder.normalize_json(v)) unless v.nil?
      end
      out
    else
      value
    end
  end

  # `store_nested_param`, rule for rule. Answers what Rails' answers —
  # `params`, a one-element Array for a trailing `[]` below the top, or
  # nil for an empty key — because callers store that answer.
  def self.store(params, name, v, depth)
    raise ParamBuilder::Invalid if depth >= DEPTH_LIMIT
    k = ""
    after = ""
    if depth == 0
      start = name.length > 1 ? name.index("[", 1) : nil
      if start.nil?
        k = name
      else
        k = name[0, start].to_s
        after = name[start, name.length - start].to_s
      end
    elsif name.start_with?("[]")
      k = "[]"
      after = name[2, name.length - 2].to_s
    elsif name.start_with?("[") && !(close = name.index("]", 1)).nil?
      k = name[1, close - 1].to_s
      after = name[close + 1, name.length - close - 1].to_s
    else
      k = name
    end

    return nil if k.empty?
    raise ParamBuilder::Invalid unless k.valid_encoding?
    raise ParamBuilder::Invalid if depth == 0 && !v.nil? && !v.valid_encoding?

    if after == ""
      if k == "[]" && depth != 0
        return v.nil? ? [] : [v]
      end
      params[k] = v
    elsif after == "["
      params[name] = v
    elsif after == "[]"
      params[k] = [] if params[k].nil?
      list = params[k]
      raise ParamBuilder::Invalid unless list.is_a?(Array)
      list.push(v) unless v.nil?
    elsif after.start_with?("[]")
      child = ParamBuilder.hash_in_array_key(after)
      child = after[2, after.length - 2].to_s if child.nil?
      params[k] = [] if params[k].nil?
      list = params[k]
      raise ParamBuilder::Invalid unless list.is_a?(Array)
      last = list.last
      if last.is_a?(Hash) && !ParamBuilder.has_key_path?(last, child)
        ParamBuilder.store(last, child, v, depth + 1)
      else
        list.push(ParamBuilder.store({}, child, v, depth + 1))
      end
    else
      params[k] = {} if params[k].nil?
      sub = params[k]
      raise ParamBuilder::Invalid unless sub.is_a?(Hash)
      params[k] = ParamBuilder.store(sub, after, v, depth + 1)
    end
    params
  end

  # `x[][y]`: the `y` of a hash inside an array, when `after` is exactly
  # `[][y]` with no further brackets in `y`; nil otherwise.
  def self.hash_in_array_key(after)
    return nil unless after.length > 4 && after[2] == "[" && after.end_with?("]")
    child = after[3, after.length - 4].to_s
    return nil if child.empty? || !child.index("[").nil? || !child.index("]").nil?
    child
  end

  # `params_hash_has_key?`: does `hash` already hold the bracket path
  # `key` (`b`, `b][c`, …)? A path through `[]` never counts as held.
  def self.has_key_path?(hash, key)
    return false unless key.index("[]").nil?
    h = hash
    parts = key.split(/[\[\]]+/)
    i = 0
    while i < parts.length
      part = parts[i]
      if part != ""
        return false unless h.is_a?(Hash) && h.key?(part)
        h = h[part]
      end
      i += 1
    end
    true
  end
end
