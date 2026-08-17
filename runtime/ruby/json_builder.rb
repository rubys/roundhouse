# JSON encoding primitives used by Views::*.<x>_json render methods —
# the lowered output of `*.json.jbuilder` templates. Hand-rolled to drop
# the stdlib `json` dependency (which spinel doesn't ship), and to keep
# the surface small enough that the same Ruby transpiles cleanly to
# every Group 1 target.
#
# Scope: three primitives the Jbuilder lowerer needs for real-blog
# templates — encode_string (RFC 8259 escaping for the common cases),
# encode_value (type-dispatched scalar encoder), and encode_datetime
# (Rails-compatible ISO 8601 reformat for `datetime` columns). The
# lowerer inlines `{`/`,`/`}` and `[`/`,`/`]` directly into method
# bodies, so the runtime has no array_join / object_pairs primitive
# today; those land when stretch DSL forms (json.merge!, dynamic-shape
# objects) need them.
#
# Decimal handling is still the lowerer's job: call sites pass strings
# or pre-formatted values. encode_datetime is bundled here because the
# input is uniformly a sqlite-shape TEXT timestamp and the output
# format is Rails-canonical; doing the reformat in the runtime keeps
# the lowerer's column-aware routing simple (just "is this column a
# datetime, yes or no").
#
# The `read_*` / `write_*` half is the DECODING side, added for
# `has_json :settings, key: default` (ActiveModel::SchematizedJson):
# a flat JSON object living in one column, whose keys and scalar types
# are known at transpile time. `lower::has_json` synthesizes one typed
# accessor triple per schema key and routes it through here, so the
# module is the app's one JSON home rather than a second one growing
# next to it. Only FLAT objects of scalar values are handled — which is
# the whole of what `has_json` allows ("Only the three basic JSON
# types are supported: boolean, integer, and string. No nesting
# either.").
module JsonBuilder
  ESCAPES = {
    "\\" => "\\\\",
    "\"" => "\\\"",
    "\n" => "\\n",
    "\r" => "\\r",
    "\t" => "\\t",
    "\b" => "\\b",
    "\f" => "\\f",
  }.freeze

  # `\x08` rather than `\b` for the backspace because Rust's `regex`
  # crate rejects `\b` inside a character class (where it would
  # otherwise be word-boundary, which makes no sense inside `[]`).
  # Ruby/JS/Crystal/RE2 all accept the hex escape, so this is the
  # cross-target spelling.
  ESCAPE_PATTERN = /[\\"\n\r\t\x08\f]/.freeze

  # Escape a string for embedding inside JSON double-quotes. Does
  # NOT add the surrounding quotes — `encode_value` wraps a String
  # value in quotes; callers building object keys interpolate the
  # raw escape result inside their own `"…"`.
  #
  # Non-nil contract: callers (encode_value, encode_datetime, lowered
  # template bodies) narrow nil before reaching here. Strict-typed
  # targets (Rust, Crystal) compile against `String` directly without
  # Option-wrapping at every call site.
  def self.encode_string(s)
    s.gsub(ESCAPE_PATTERN, ESCAPES)
  end

  # Render a scalar Ruby value as its JSON fragment, complete with
  # surrounding quotes for strings. Returns a String the lowered
  # body can concatenate directly into the io accumulator.
  def self.encode_value(v)
    return "null" if v.nil?
    return "true" if v.is_a?(TrueClass)
    return "false" if v.is_a?(FalseClass)
    return v.to_s if v.is_a?(Integer)
    return v.to_s if v.is_a?(Float)
    return "\"#{encode_string(v)}\"" if v.is_a?(String)
    # Fallback: stringify and quote. Call sites convert Time /
    # BigDecimal / etc. before reaching here.
    "\"#{encode_string(v.to_s)}\""
  end

  # Reformat a sqlite-shape TEXT timestamp ("YYYY-MM-DD HH:MM:SS[.f]")
  # to Rails-canonical ISO 8601 with millisecond precision and a `Z`
  # suffix ("YYYY-MM-DDTHH:MM:SS.fffZ"). Returns a JSON-quoted string.
  # Inputs that don't match the expected shape pass through as plain
  # quoted strings, so the call site degrades gracefully if a column
  # the lowerer routed here turns out to hold non-timestamp text.
  #
  # Assumes UTC — adapters that store local-time timestamps without
  # an offset can't be reliably normalized without per-app config; the
  # Rails default is UTC for ActiveRecord-managed datetime columns,
  # which is what real-blog produces.
  def self.encode_datetime(s)
    return "null" if s.nil?
    # A `Time` value never reaches this primitive: the Jbuilder lowerer
    # routes a temporal column through its `<col>_raw` storage reader
    # (the stored ISO-8601 TEXT), never the parsing `<col>` reader. The
    # string→string reformat is exact — no float sub-second hazards —
    # and skips a native parse→format round-trip per row. Keeping `Time`
    # out of this file is also what lets it transpile cleanly to targets
    # with no `Time` type.
    str = s.to_s
    return "\"#{encode_string(str)}\"" if str.length < 19
    date = str[0, 10]
    time = str[11, 8]
    ms = "000"
    if str.length > 20 && str[19, 1] == "."
      # `str[20..]` (open-ended) rather than `str[20..-1]`. Both
      # forms now lower correctly on every target — the TS emit's
      # `Range { end: -1, inclusive }` path was fixed to produce
      # `str.slice(20)` instead of the old `str.slice(20, -1 + 1)`
      # = `str.slice(20, 0)` = empty (zeroed-out fractional
      # seconds). Keep the open-ended idiom: it's the Ruby 2.6+
      # convention and the lowering is unambiguous.
      frac = str[20..]
      padded = "#{frac}000"
      ms = padded[0, 3]
    end
    "\"#{date}T#{time}.#{ms}Z\""
  end

  # --- Schematized-JSON column decoding ------------------------------
  #
  # `has_json :settings, restrict_creation: false` stores a FLAT object
  # of scalar values in one column. The declaration names every key and
  # its type at transpile time, so `lower::has_json` emits one typed
  # accessor triple per key and this half only has to move single
  # values in and out of the serialized text.

  # Parse a flat JSON object into `key => the value's SOURCE TEXT`
  # (`"true"`, `"42"`, `"\"hello\""`, `"null"`).
  #
  # Values stay as source text on purpose: that keeps the map
  # homogeneous — `Hash[String, String]` on every target, one element
  # type per container — while still round-tripping a key this process
  # never decoded. `read_*` decodes one value against its declared
  # type; `write_*` re-encodes one and re-serializes the rest verbatim.
  #
  # A nil / blank / `{}` column parses to an empty hash. Nested objects
  # and arrays are neither what `has_json` permits nor what `write_*`
  # produces, so a value opening with `{` or `[` ENDS the scan rather
  # than being mis-split on its inner commas — a truncated read of
  # foreign JSON, never a silently wrong one.
  #
  # Loop exits are flag-guarded rather than `break`, and iteration is
  # index-based rather than `each`: this file is one of the stems every
  # target transpiles, and `Break` / each-blocks do not lower to python
  # ([[feedback_cross_target_safe_runtime_idioms]]).
  def self.parse_object(serialized)
    out = {}
    s = serialized.to_s
    n = s.length
    i = 0
    while i < n && s[i, 1].to_s != "{"
      i = i + 1
    end
    i = i + 1
    scanning = true
    while scanning
      while i < n && json_separator?(s[i, 1].to_s)
        i = i + 1
      end
      if i >= n || s[i, 1].to_s != "\""
        scanning = false
      else
        key_end = scan_json_string(s, i)
        key = decode_string(s[i, key_end - i].to_s)
        i = key_end
        while i < n && s[i, 1].to_s != ":"
          i = i + 1
        end
        i = i + 1
        while i < n && json_separator?(s[i, 1].to_s)
          i = i + 1
        end
        value_end = scan_json_value(s, i)
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

  # Whitespace or a structural comma — everything that may sit between
  # a flat object's tokens.
  def self.json_separator?(c)
    c == " " || c == "\n" || c == "\t" || c == "\r" || c == ","
  end

  # Index just PAST the closing quote of the string starting at `start`
  # (which must be its opening quote), or the end of the input when the
  # string is unterminated. Backslash escapes are skipped as a unit so
  # an escaped quote doesn't end the scan early.
  #
  # Returns `i` in every path, including the unterminated one: `s.length`
  # and `start + 1` are not the same integer width on crystal, and a
  # method whose two exits disagree doesn't compile there.
  def self.scan_json_string(s, start)
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
  def self.scan_json_value(s, start)
    return scan_json_string(s, start) if s[start, 1].to_s == "\""
    n = s.length
    i = start
    done = false
    while i < n && !done
      c = s[i, 1].to_s
      if c == "," || c == "}" || c == "{" || c == "[" ||
          c == " " || c == "\n" || c == "\t" || c == "\r"
        done = true
      else
        i = i + 1
      end
    end
    i
  end

  # The Ruby String behind a JSON string literal (quotes included).
  # The inverse of `encode_string` plus its quotes; `\uXXXX` is left
  # verbatim, which `encode_string` never emits.
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

  # The value's SOURCE TEXT for `key`, or `""` when the object carries
  # no such key. Empty is unambiguous: the shortest JSON value text is
  # two characters, so nothing valid renders as the empty string.
  #
  # `data` is a PARAMETER rather than a local assigned from
  # `parse_object`. Every target's Hash surface — `key?` → `in` /
  # `has_key?`, `keys` → `Object.keys` — is selected by the RECEIVER's
  # declared type, and the runtime transpile types signatures from the
  # `.rbs`, not locals inferred from a call. A local receiver therefore
  # emits `data.is_key(k)` and `data.keys` as property reads; a typed
  # param emits the real thing.
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
  # object — the caller assigns it back to the column ivar (same shape
  # as `TypedStore.write`, which is the YAML twin of this seam).
  def self.write_boolean(serialized, key, value)
    write_json_raw(serialized, key, value ? "true" : "false")
  end

  def self.write_integer(serialized, key, value)
    write_json_raw(serialized, key, value.to_s)
  end

  def self.write_string(serialized, key, value)
    write_json_raw(serialized, key, "\"#{encode_string(value)}\"")
  end

  # Keys are emitted in sorted order rather than parse order: Ruby's
  # Hash preserves insertion, but not every target's does, and a
  # column's bytes should not depend on which one wrote them.
  def self.write_json_raw(serialized, key, raw)
    data = parse_object(serialized)
    data[key] = raw
    render_object(data)
  end

  # Serialize a parsed object back to JSON text. A typed param for the
  # same reason `read_raw` takes one — see there.
  def self.render_object(data)
    names = data.keys.sort
    out = "{"
    i = 0
    while i < names.length
      out = out + "," if i > 0
      out = out + "\"" + encode_string(names[i].to_s) + "\":" + read_raw(data, names[i].to_s)
      i = i + 1
    end
    out + "}"
  end
end
