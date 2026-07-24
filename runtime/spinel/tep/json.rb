# Tep::Json -- a small JSON encoder + flat-key decoder for
# spinel-AOT'd apps.
#
# Why bundle one? The stdlib `json` gem's fast path is a CRuby
# native extension (`JSON::Ext`); the pure-Ruby fallback
# (`JSON::Pure`) is heavily metaprogrammed (`define_method` for
# state-machine transitions, `class_eval`'d generator dispatch),
# which spinel can't lower. The `oj` / `yajl-ruby` / `multi_json`
# alternatives are all C extensions or thin wrappers thereof.
#
# Scope
# -----
# This is a *batteries-included for JSON-over-HTTP* shim, not a
# full JSON library:
#
#   * **Encode**: produce JSON strings from typed Ruby values --
#     `escape(s)` / `quote(s)` for strings; `encode_pair_str(k, v)`
#     and `encode_pair_int(k, v)` as fixed-arity building blocks
#     for object literals; `from_str_array(a)` and
#     `from_int_array(a)` for array literals. Compose objects in
#     user code by concatenation:
#
#         "{" + Tep::Json.encode_pair_str("name", name) + "," +
#               Tep::Json.encode_pair_int("age", age) + "}"
#
#   * **Decode**: typed accessors that read a *top-level key* from
#     a flat JSON object: `get_str`, `get_int`, `has_key?`. The
#     parser walks the object once, skipping nested values without
#     materialising them. For deeper traversal users hand-write
#     the walk; nested JSON-object access of the form
#     `payload.user.email` should be done by the API contract:
#     pass discrete fields rather than a nested blob.
#
# Out of scope
# ------------
#   * Floats. Numbers parse / emit as int (`.to_s`). JSON's number
#     grammar is wider but APIs in practice send integers for IDs,
#     counts, timestamps; treat anything fractional as a string in
#     transport.
#   * Unicode-escape decoding (`\uXXXX`). Round-trips through
#     escape/unescape work for ASCII; non-ASCII input bytes are
#     passed through verbatim.
#   * Streaming / pull parsers. Loads the whole string into the
#     parser; suitable for request-body sizes typical of APIs.
module Tep
  class Json
    # ---- Encoders ----

    # Escape a string for inclusion inside a JSON string literal
    # (does NOT add the surrounding quotes -- use `quote(s)` for
    # that). Handles ", \, and the JSON-required control-char
    # escapes (\b, \f, \n, \r, \t); other control bytes go through
    # \u00XX. Forward slash is left unescaped (legal either way;
    # the unescaped form is more readable and shorter).
    def self.escape(s)
      out = +""
      i = 0
      n = s.length
      while i < n
        c = s[i]
        if c == "\""
          out << "\\\""
        elsif c == "\\"
          out << "\\\\"
        elsif c == "\n"
          out << "\\n"
        elsif c == "\r"
          out << "\\r"
        elsif c == "\t"
          out << "\\t"
        elsif c == "\b"
          out << "\\b"
        elsif c == "\f"
          out << "\\f"
        elsif c < " "
          # Other control byte -- emit \u00XX. c.bytes[0] is the
          # raw byte value, mapped to two hex digits.
          b = c.bytes[0]
          out << "\\u00" + Json.hex2(b)
        else
          out << c
        end
        i += 1
      end
      out
    end

    # Two-digit lowercase hex of a byte (0..255).
    def self.hex2(n)
      hex = "0123456789abcdef"
      out = +""
      out << hex[(n / 16) % 16, 1]
      out << hex[n % 16, 1]
      out
    end

    # Wrap a string in JSON quotes, escaping its body.
    def self.quote(s)
      "\"" + Json.escape(s) + "\""
    end

    # Encode a single key/value pair as `"k":"v"` (escaped both
    # sides). Building block for ad-hoc object literals where the
    # caller wants control over key ordering or layout:
    #
    #   "{" + Tep::Json.encode_pair_str("name", name) + "," +
    #         Tep::Json.encode_pair_int("age", age) + "}"
    #
    # When you have a real Hash, prefer `from_str_hash` /
    # `from_int_hash` -- those iterate via `each |k, v|` directly.
    def self.encode_pair_str(k, v)
      Json.quote(k) + ":" + Json.quote(v)
    end

    # Same shape, integer value side. `v` is rendered via `.to_s`
    # so JSON-numeric output without quoting.
    def self.encode_pair_int(k, v)
      Json.quote(k) + ":" + v.to_s
    end

    # Encode a Hash<String,String> as a JSON object.
    def self.from_str_hash(h)
      out = "{"
      first = true
      h.each do |k, v|
        if !first
          out << ","
        end
        first = false
        out << Json.quote(k) + ":" + Json.quote(v)
      end
      out + "}"
    end

    # Same shape with integer values. JSON-numeric, no quoting.
    def self.from_int_hash(h)
      out = "{"
      first = true
      h.each do |k, v|
        if !first
          out << ","
        end
        first = false
        out << Json.quote(k) + ":" + v.to_s
      end
      out + "}"
    end

    # Encode a string array as a JSON array of quoted strings.
    def self.from_str_array(a)
      out = "["
      i = 0
      while i < a.length
        if i > 0
          out << ","
        end
        out << Json.quote(a[i])
        i += 1
      end
      out + "]"
    end

    # Encode an int array as a JSON array of numbers.
    def self.from_int_array(a)
      out = "["
      i = 0
      while i < a.length
        if i > 0
          out << ","
        end
        out << a[i].to_s
        i += 1
      end
      out + "]"
    end

    # ---- Decoders (flat-key, top-level only) ----
    #
    # `get_str(s, key)` finds the entry for `key` in the top-level
    # object literal `s` and returns its value as a string.
    # Returns "" when `key` is absent or the value isn't a string.
    # Same shape for `get_int`. `has_key?(s, key)` returns a
    # boolean independent of value type.
    #
    # The parser is a hand-rolled state machine that walks one
    # `{ "k": <value>, ... }` pair at a time, skipping over any
    # value (including nested objects / arrays) it doesn't need.
    # Strings inside values are honoured for escape sequences so
    # that `\"` doesn't terminate the string and corrupt the walk.

    def self.get_str(s, key)
      pos = Json.find_value_start(s, key)
      if pos < 0
        return ""
      end
      Json.parse_str_value(s, pos)
    end

    def self.get_int(s, key)
      pos = Json.find_value_start(s, key)
      if pos < 0
        return 0
      end
      Json.parse_int_value(s, pos)
    end

    # Decode a JSON number value at `key` -> Float. Accepts both
    # integer-literal (`42`) and float-literal (`3.14`, `-0.5`, `1e2`)
    # JSON-number syntax; the integer form returns N.0. Missing key
    # or malformed value returns 0.0 (consistent with the other
    # getters' missing-key defaults).
    #
    # Implementation: delegates the value-span walking to skip_value
    # (already handles all JSON-number syntax + structural-char
    # boundaries), then String#to_f on the substring. Inlined rather
    # than factored into a parse_float_value helper because spinel's
    # type inference mis-widens `s` to int through the indirection
    # ("cannot resolve call to 'length' on int" + the downstream
    # skip_ws/skip_value pointer-vs-int conversion errors).
    def self.get_float(s, key)
      pos = Json.find_value_start(s, key)
      if pos < 0
        return 0.0
      end
      pos = Json.skip_ws(s, pos)
      if pos >= s.length
        return 0.0
      end
      end_pos = Json.skip_value(s, pos)
      if end_pos <= pos
        return 0.0
      end
      s[pos, end_pos - pos].to_f
    end

    def self.has_key?(s, key)
      Json.find_value_start(s, key) >= 0
    end

    # Decode a flat JSON array of integers at `key` -> Array[Integer].
    # The `prompt` of /v1/completions is a token-id array
    # (`[464, 6193, ...]`). A missing or non-array value yields []
    # (the tep typed-empty-array idiom); non-int elements are skipped.
    def self.get_int_array(s, key)
      out = [0]
      out.pop
      pos = Json.find_value_start(s, key)
      if pos < 0
        return out
      end
      pos = Json.skip_ws(s, pos)
      if pos >= s.length || s[pos] != "["
        return out
      end
      pos += 1
      while pos < s.length
        pos = Json.skip_ws(s, pos)
        if pos >= s.length
          return out
        end
        c = s[pos]
        if c == "]"
          return out
        elsif c == ","
          pos += 1
        elsif (c >= "0" && c <= "9") || c == "-"
          out.push(Json.parse_int_value(s, pos))
          # Advance past the number parse_int_value just consumed
          # (optional '-' then digits).
          if s[pos] == "-"
            pos += 1
          end
          while pos < s.length && s[pos] >= "0" && s[pos] <= "9"
            pos += 1
          end
        else
          # Non-int element (string / object / etc.): skip it.
          pos = Json.skip_value(s, pos)
        end
      end
      out
    end

    # ---- Internal helpers ----

    # Skip whitespace starting at `pos`, return the new position.
    def self.skip_ws(s, pos)
      while pos < s.length
        c = s[pos]
        if c == " " || c == "\t" || c == "\n" || c == "\r"
          pos += 1
        else
          return pos
        end
      end
      pos
    end

    # Walk a JSON-quoted string starting at `pos` (which must point
    # at the opening `"`). Returns the position one past the
    # closing `"`. Returns -1 on malformed input.
    def self.skip_str(s, pos)
      if pos >= s.length || s[pos] != "\""
        return -1
      end
      pos += 1
      while pos < s.length
        c = s[pos]
        if c == "\\"
          # Skip the escape and the escaped character. \uXXXX
          # spans 6 chars total but skipping 2 still keeps us
          # inside the string for the rest of the walk -- the
          # remaining 4 hex digits look like ordinary string
          # bytes and won't terminate the literal.
          pos += 2
        elsif c == "\""
          return pos + 1
        else
          pos += 1
        end
      end
      -1
    end

    # Walk a JSON value starting at `pos` (which must point at the
    # first non-ws char of the value). Returns the position one
    # past the value (or the input length on truncation).
    def self.skip_value(s, pos)
      pos = Json.skip_ws(s, pos)
      if pos >= s.length
        return pos
      end
      c = s[pos]
      if c == "\""
        return Json.skip_str(s, pos)
      end
      if c == "{" || c == "["
        return Json.skip_container(s, pos)
      end
      # number / true / false / null -- read until the next
      # structural / whitespace char.
      while pos < s.length
        c = s[pos]
        if c == "," || c == "}" || c == "]" ||
           c == " " || c == "\t" || c == "\n" || c == "\r"
          return pos
        end
        pos += 1
      end
      pos
    end

    # Walk a balanced { ... } or [ ... ] starting at `pos`. Honours
    # string literals so that `{` / `}` inside a value-string don't
    # confuse the brace counter. Returns position one past the
    # matching closer.
    def self.skip_container(s, pos)
      open_c = s[pos]
      close_c = open_c == "{" ? "}" : "]"
      depth = 1
      pos += 1
      while pos < s.length && depth > 0
        c = s[pos]
        if c == "\""
          # whole nested string -- skip past it
          npos = Json.skip_str(s, pos)
          if npos < 0
            return s.length
          end
          pos = npos
        elsif c == open_c
          depth += 1
          pos += 1
        elsif c == close_c
          depth -= 1
          pos += 1
        else
          pos += 1
        end
      end
      pos
    end

    # Read a JSON-quoted string at `pos` and return its decoded
    # contents (no surrounding quotes). Decodes the same escape
    # sequences that `escape` produces. Returns "" on malformed
    # input.
    def self.parse_str_value(s, pos)
      pos = Json.skip_ws(s, pos)
      if pos >= s.length || s[pos] != "\""
        return ""
      end
      pos += 1
      out = +""
      while pos < s.length
        c = s[pos]
        if c == "\""
          return out
        end
        if c == "\\"
          if pos + 1 >= s.length
            return out
          end
          esc = s[pos + 1]
          if esc == "\""
            out << "\""
          elsif esc == "\\"
            out << "\\"
          elsif esc == "/"
            out << "/"
          elsif esc == "n"
            out << "\n"
          elsif esc == "r"
            out << "\r"
          elsif esc == "t"
            out << "\t"
          elsif esc == "b"
            out << "\b"
          elsif esc == "f"
            out << "\f"
          elsif esc == "u"
            # \u00XX -> map the two-digit hex back to a byte. Wider
            # codepoints (Ā+ or surrogate pairs) aren't
            # decoded; the byte we emit is the low byte of the
            # codepoint, which round-trips ASCII at minimum.
            if pos + 5 < s.length
              h1 = Json.hex_nibble(s[pos + 4])
              h2 = Json.hex_nibble(s[pos + 5])
              if h1 >= 0 && h2 >= 0
                # rebuild the byte and push it -- spinel strings
                # are byte-blobs, so this works for ASCII; for
                # non-ASCII the original encoder would have used a
                # passthrough byte anyway.
                b = h1 * 16 + h2
                out << Json.byte_to_chr(b)
                pos += 6
                next
              end
            end
            out << "?"
            pos += 2
            next
          else
            out << esc
          end
          pos += 2
        else
          out << c
          pos += 1
        end
      end
      out
    end

    def self.hex_nibble(c)
      if c >= "0" && c <= "9"
        return c.bytes[0] - "0".bytes[0]
      end
      if c >= "a" && c <= "f"
        return c.bytes[0] - "a".bytes[0] + 10
      end
      if c >= "A" && c <= "F"
        return c.bytes[0] - "A".bytes[0] + 10
      end
      -1
    end

    # Build a single-byte string from an integer 0..255.
    # Spinel doesn't expose `n.chr` for arbitrary bytes uniformly;
    # the table covers the ASCII printable range and falls back to
    # "?" for anything else (the JSON encoder side never produces
    # non-ASCII via \u, so the fallback is reachable only for
    # malformed input).
    def self.byte_to_chr(n)
      printable = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~"
      if n >= 32 && n < 127
        return printable[n - 32, 1]
      end
      if n == 9
        return "\t"
      end
      if n == 10
        return "\n"
      end
      if n == 13
        return "\r"
      end
      "?"
    end

    # Read an integer at `pos`. Accepts an optional leading `-`.
    # Returns 0 on no-digit / non-numeric input (caller can use
    # `has_key?` first if 0-vs-absent matters).
    def self.parse_int_value(s, pos)
      pos = Json.skip_ws(s, pos)
      if pos >= s.length
        return 0
      end
      neg = false
      if s[pos] == "-"
        neg = true
        pos += 1
      end
      n = 0
      saw_digit = false
      while pos < s.length
        c = s[pos]
        if c >= "0" && c <= "9"
          n = n * 10 + (c.bytes[0] - "0".bytes[0])
          saw_digit = true
          pos += 1
        else
          break
        end
      end
      if !saw_digit
        return 0
      end
      neg ? -n : n
    end

    # Walk the top-level object looking for the entry whose key
    # matches `target_key`; return the position of the value's
    # first non-ws character. Returns -1 if not found.
    def self.find_value_start(s, target_key)
      pos = Json.skip_ws(s, 0)
      if pos >= s.length || s[pos] != "{"
        return -1
      end
      pos += 1
      while pos < s.length
        pos = Json.skip_ws(s, pos)
        if pos >= s.length
          return -1
        end
        if s[pos] == "}"
          return -1
        end
        # Read a key.
        if s[pos] != "\""
          return -1
        end
        key_start = pos
        pos = Json.skip_str(s, pos)
        if pos < 0
          return -1
        end
        # Decode the key for comparison (handles \" inside keys).
        key = Json.parse_str_value(s, key_start)
        # Skip ws, ":".
        pos = Json.skip_ws(s, pos)
        if pos >= s.length || s[pos] != ":"
          return -1
        end
        pos += 1
        pos = Json.skip_ws(s, pos)
        if key == target_key
          return pos
        end
        # Skip the value, then the comma (if any).
        pos = Json.skip_value(s, pos)
        pos = Json.skip_ws(s, pos)
        if pos < s.length && s[pos] == ","
          pos += 1
        elsif pos < s.length && s[pos] == "}"
          return -1
        end
      end
      -1
    end
  end
end
