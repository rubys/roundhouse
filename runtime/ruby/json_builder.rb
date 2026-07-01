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
    # A `Time` value never reaches this shared primitive: it takes the
    # stored ISO-8601 TEXT form of a datetime column (the representation
    # every non-Ruby target uses). CRuby/JRuby model accessors return a
    # real `Time` (see `apply_datetime_lowering`), so the Ruby tree
    # shadows this method with a Time-aware version in the CRuby overlay
    # (`ruby_overlay/runtime/json_builder_time.rb`). Keeping `Time` out
    # of this file is what lets it transpile cleanly to targets with no
    # `Time` type.
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
end
