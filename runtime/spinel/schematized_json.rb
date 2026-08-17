# ActiveModel::SchematizedJson subset — the JSON twin of
# `runtime/typed_store.rb`, and deliberately in the same place for the
# same reason.
#
# `has_json :settings, restrict_room_creation_to_administrators: false`
# stores a FLAT object of scalar values in one column. `lower::has_json`
# expands the declaration into one typed accessor triple per schema key
# and routes each through this module — ONE named runtime seam, resolved
# on the CRuby/JRuby/spinel trees (this file plus its overlay twin) and
# carried unresolved by the strict targets until a native implementation
# lands. Same posture as TypedStore and Duration.
#
# Why not `runtime/ruby/json_builder.rb`, where it would transpile to
# every target for free: it was there, and four targets could not
# compile it. python has no `?` in an identifier, C# read the `?`
# predicate as a nullable-coalescing operator and inferred
# `Dictionary<object?, string>` for the accumulator, kotlin's compile
# failed outright, and rust2 rendered a `Hash[String, String]` index read
# as an `Option` while emitting a panicking non-Option. Those are real
# emitter gaps and they are worth closing — but each is a separate piece
# of work, and this seam does not have to wait on all four.
#
# Scope is exactly what `has_json` permits: "Only the three basic JSON
# types are supported: boolean, integer, and string. No nesting either."
# A value opening with `{` or `[` therefore ENDS the scan rather than
# being mis-split on its inner commas — a truncated read of foreign
# JSON, never a silently wrong one.
#
# String escaping is `JsonBuilder.encode_string`'s, not a second copy:
# the boot chain requires `runtime/json_builder` well before this file.
module SchematizedJson
  # Parse a flat JSON object into `key => the value's SOURCE TEXT`
  # (`"true"`, `"42"`, `"\"hello\""`, `"null"`).
  #
  # Values stay as source text so the map is homogeneous — one element
  # type per container — while still round-tripping a key this process
  # never decoded. `read_*` decodes one value against its declared type;
  # `write_*` re-encodes one and re-serializes the rest verbatim.
  def self.parse_object(serialized)
    out = {}
    return out if serialized.nil?
    s = serialized.to_s
    n = s.length
    i = 0
    while i < n && s[i, 1].to_s != "{"
      i = i + 1
    end
    i = i + 1
    scanning = true
    while scanning
      while i < n && separator?(s[i, 1].to_s)
        i = i + 1
      end
      if i >= n || s[i, 1].to_s != "\""
        scanning = false
      else
        key_end = scan_string(s, i)
        key = decode_string(s[i, key_end - i].to_s)
        i = key_end
        while i < n && s[i, 1].to_s != ":"
          i = i + 1
        end
        i = i + 1
        while i < n && separator?(s[i, 1].to_s)
          i = i + 1
        end
        value_end = scan_value(s, i)
        if i >= n || value_end == i
          scanning = false
        else
          out[key] = s[i, value_end - i].to_s
          i = value_end
        end
      end
    end
    out
  end

  # Whitespace or a structural comma — everything that may sit between a
  # flat object's tokens.
  def self.separator?(c)
    c == " " || c == "\n" || c == "\t" || c == "\r" || c == ","
  end

  # Index just PAST the closing quote of the string starting at `start`
  # (its opening quote), or the end of the input when unterminated.
  # Backslash escapes are skipped as a unit so an escaped quote doesn't
  # end the scan early.
  def self.scan_string(s, start)
    n = s.length
    i = start + 1
    done = false
    while i < n && !done
      c = s[i, 1].to_s
      if c == "\\"
        i = i + 2
      elsif c == "\""
        i = i + 1
        done = true
      else
        i = i + 1
      end
    end
    i
  end

  # Index just past the value starting at `start`.
  def self.scan_value(s, start)
    return scan_string(s, start) if s[start, 1].to_s == "\""
    n = s.length
    i = start
    done = false
    while i < n && !done
      c = s[i, 1].to_s
      if c == "," || c == "}" || c == "{" || c == "["
        done = true
      elsif c == " " || c == "\n" || c == "\t" || c == "\r"
        done = true
      else
        i = i + 1
      end
    end
    i
  end

  # The Ruby String behind a JSON string literal (quotes included) — the
  # inverse of `JsonBuilder.encode_string` plus its quotes. `\uXXXX` is
  # left verbatim, which that escaper never emits.
  def self.decode_string(raw)
    s = raw.to_s
    return s if s.length < 2 || s[0, 1].to_s != "\""
    out = ""
    i = 1
    last = s.length - 1
    while i < last
      c = s[i, 1].to_s
      if c == "\\"
        e = s[i + 1, 1].to_s
        if e == "n"
          out = out + "\n"
        elsif e == "r"
          out = out + "\r"
        elsif e == "t"
          out = out + "\t"
        elsif e == "b"
          out = out + "\b"
        elsif e == "f"
          out = out + "\f"
        else
          out = out + e
        end
        i = i + 2
      else
        out = out + c
        i = i + 1
      end
    end
    out
  end

  # The value's source text for `key`, or `""` when the object carries no
  # such key. Empty is unambiguous: the shortest JSON value text is two
  # characters, so nothing valid renders as the empty string.
  def self.read_raw(data, key)
    return "" if !data.key?(key)
    data[key].to_s
  end

  # Typed single-key reads. `fallback` answers both an absent key and a
  # stored `null`: these readers are declared to return the schema's
  # scalar type, and Rails' own accessor reverse-merges the declared
  # default over missing data before anything reads it.
  def self.read_boolean(serialized, key, fallback)
    raw = read_raw(parse_object(serialized), key)
    return fallback if raw == "" || raw == "null"
    raw == "true"
  end

  def self.read_integer(serialized, key, fallback)
    raw = read_raw(parse_object(serialized), key)
    return fallback if raw == "" || raw == "null"
    raw.to_i
  end

  def self.read_string(serialized, key, fallback)
    raw = read_raw(parse_object(serialized), key)
    return fallback if raw == "" || raw == "null"
    decode_string(raw)
  end

  # Typed single-key writes, each returning the WHOLE re-serialized
  # object — the caller assigns it back to the column ivar, the same
  # shape as `TypedStore.write`.
  def self.write_boolean(serialized, key, value)
    write_raw(serialized, key, value ? "true" : "false")
  end

  def self.write_integer(serialized, key, value)
    write_raw(serialized, key, value.to_s)
  end

  def self.write_string(serialized, key, value)
    write_raw(serialized, key, "\"" + JsonBuilder.encode_string(value) + "\"")
  end

  def self.write_raw(serialized, key, raw)
    data = parse_object(serialized)
    data[key] = raw
    render_object(data)
  end

  # Serialize a parsed object back to JSON text. Keys are emitted in
  # SORTED order rather than parse order: Ruby's Hash preserves
  # insertion, but not every target's does, and a column's bytes should
  # not depend on which one wrote them.
  def self.render_object(data)
    names = data.keys.sort
    out = "{"
    i = 0
    while i < names.length
      out = out + "," if i > 0
      out = out + "\"" + JsonBuilder.encode_string(names[i].to_s) + "\":" +
        read_raw(data, names[i].to_s)
      i = i + 1
    end
    out + "}"
  end
end
