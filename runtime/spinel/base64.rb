# Minimal Base64 shim — provides only the surface called from
# framework Ruby. The Ruby target's `require_relative "runtime/base64"`
# resolves here uniformly; under CRuby this module is also reachable
# via the stdlib's `require "base64"` but the framework Ruby's
# `Base64.strict_encode64` call site doesn't depend on which path
# loaded the constant — both implementations produce the same RFC
# 4648 strict (no-line-wrap) encoding.
#
# Currently only `strict_encode64` is needed (turbo-stream signed-name
# encoding from `runtime/ruby/action_view/view_helpers.rb`). The
# broader stdlib surface (encode64 with line wrapping, decode64,
# urlsafe_*, etc.) is omitted because no caller exists; add on
# demand.
module Base64
  ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"

  # RFC 4648 §4 base64 encoding with no line wrapping. Iterates the
  # input bytes in 3-byte chunks producing 4 output characters per
  # chunk; trailing 1- or 2-byte remainders pad with "=" / "==".
  def self.strict_encode64(s)
    bytes = s.bytes
    n = bytes.length
    out = ""
    i = 0
    while i + 3 <= n
      b0 = bytes[i]
      b1 = bytes[i + 1]
      b2 = bytes[i + 2]
      out = out + ALPHABET[(b0 >> 2) & 0x3F].to_s
      out = out + ALPHABET[((b0 << 4) | (b1 >> 4)) & 0x3F].to_s
      out = out + ALPHABET[((b1 << 2) | (b2 >> 6)) & 0x3F].to_s
      out = out + ALPHABET[b2 & 0x3F].to_s
      i = i + 3
    end
    rem = n - i
    if rem == 1
      b0 = bytes[i]
      out = out + ALPHABET[(b0 >> 2) & 0x3F].to_s
      out = out + ALPHABET[(b0 << 4) & 0x3F].to_s
      out = out + "=="
    elsif rem == 2
      b0 = bytes[i]
      b1 = bytes[i + 1]
      out = out + ALPHABET[(b0 >> 2) & 0x3F].to_s
      out = out + ALPHABET[((b0 << 4) | (b1 >> 4)) & 0x3F].to_s
      out = out + ALPHABET[(b1 << 2) & 0x3F].to_s
      out = out + "="
    end
    out
  end
end
