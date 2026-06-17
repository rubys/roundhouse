# Minimal JSON shim — provides only the surface called from framework
# Ruby. The Ruby target's `require_relative "runtime/json"` resolves
# here uniformly; under CRuby this overrides the stdlib's
# `JSON.generate` with a semantically-equivalent implementation for
# the input shapes the framework uses.
#
# Currently only `generate` is needed, and only with String input
# (turbo-stream signed-name encoding from
# `runtime/ruby/action_view/view_helpers.rb#turbo_stream_from` —
# `JSON.generate("articles")` → `"\"articles\""`). The broader stdlib
# surface (Hash/Array/numeric/nil/bool handling, `parse`, options
# hashes, pretty-printing, etc.) is omitted because no caller exists;
# add on demand.
module JSON
  # RFC 8259 string serialization: surround with double-quotes,
  # escape the seven characters that MUST be escaped. Non-ASCII
  # passes through unchanged (the stdlib defaults to UTF-8 raw too
  # unless `ascii_only: true` is requested).
  def self.generate(value)
    if value.is_a?(String)
      "\"" + escape_string(value) + "\""
    else
      # Fallback for non-String inputs — to_s is closer to Rails'
      # actual behavior for the few non-String callers we have than
      # silently returning nil or raising.
      value.to_s
    end
  end

  def self.escape_string(s)
    out = ""
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
      else
        out << c.to_s
      end
      i = i + 1
    end
    out
  end
end
