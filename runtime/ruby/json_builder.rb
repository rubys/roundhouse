# JSON encoding primitives used by Views::*.<x>_json render methods —
# the lowered output of `*.json.jbuilder` templates. Hand-rolled to drop
# the stdlib `json` dependency (which spinel doesn't ship), and to keep
# the surface small enough that the same Ruby transpiles cleanly to
# every Group 1 target.
#
# Scope: the two primitives the Jbuilder lowerer needs for real-blog
# templates — encode_string (RFC 8259 escaping for the common cases)
# and encode_value (type-dispatched scalar encoder). The lowerer
# inlines `{`/`,`/`}` and `[`/`,`/`]` directly into method bodies,
# so the runtime has no array_join / object_pairs primitive today;
# those land when stretch DSL forms (json.merge!, dynamic-shape
# objects) need them.
#
# Timestamp / decimal handling is the lowerer's job, not the runtime's:
# call sites pass `.iso8601(3)` etc. so this module never sees Time or
# BigDecimal. Keeps the runtime free of class-name-keyed dispatch.
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

  ESCAPE_PATTERN = /[\\"\n\r\t\b\f]/.freeze

  # Escape a string for embedding inside JSON double-quotes. Does
  # NOT add the surrounding quotes — `encode_value` wraps a String
  # value in quotes; callers building object keys interpolate the
  # raw escape result inside their own `"…"`.
  def self.encode_string(s)
    return "" if s.nil?
    s.to_s.gsub(ESCAPE_PATTERN, ESCAPES)
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
end
