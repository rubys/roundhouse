# CRuby-only ActionView capture/concat: block-buffered helper output
# (Rails CaptureHelper). `capture` pushes a fresh buffer, runs the block,
# and returns what `concat` appended, falling back to the block's value
# when nothing was concatenated and the value is a String — Rails'
# buffer.presence || value shape. `concat` outside any capture raises:
# emitted views write through io<<, so a stray concat has no view buffer
# to reach, and loud beats silently dropped output.
#
# Divergence, documented: Rails' output buffer is a SafeBuffer whose <<
# escapes non-html_safe strings; this string world carries no safety
# bit, so concat appends verbatim. The exercised call sites concat only
# literals without escapable characters and helper (link_to) output, so
# rendered bytes match; a P7 per-route diff will surface any site where
# user content flows through concat.
#
# Buffer stack rides Thread.current (Puma threads must not share a
# buffer). Callers reach these via apply_helper_lowering's framework
# rewrite, same as the date helpers.
module ActionView
  module ViewHelpers
    def self.capture
      stack = Thread.current[:roundhouse_capture_stack] ||= []
      stack.push(+"")
      value = nil
      begin
        value = yield
      ensure
        buffer = stack.pop
      end
      if buffer.empty?
        value.is_a?(String) ? value : ""
      else
        buffer
      end
    end

    def self.concat(string)
      stack = Thread.current[:roundhouse_capture_stack]
      if stack.nil? || stack.empty?
        raise "concat outside capture — emitted views buffer through io<<, not concat"
      end
      stack.last << string.to_s
      string
    end
  end
end
