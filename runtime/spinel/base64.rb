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
    out = +""
    i = 0
    while i + 3 <= n
      b0 = bytes[i]
      b1 = bytes[i + 1]
      b2 = bytes[i + 2]
      out << ALPHABET[(b0 >> 2) & 0x3F].to_s
      out << ALPHABET[((b0 << 4) | (b1 >> 4)) & 0x3F].to_s
      out << ALPHABET[((b1 << 2) | (b2 >> 6)) & 0x3F].to_s
      out << ALPHABET[b2 & 0x3F].to_s
      i = i + 3
    end
    rem = n - i
    if rem == 1
      b0 = bytes[i]
      out << ALPHABET[(b0 >> 2) & 0x3F].to_s
      out << ALPHABET[(b0 << 4) & 0x3F].to_s
      out << "=="
    elsif rem == 2
      b0 = bytes[i]
      b1 = bytes[i + 1]
      out << ALPHABET[(b0 >> 2) & 0x3F].to_s
      out << ALPHABET[((b0 << 4) | (b1 >> 4)) & 0x3F].to_s
      out << ALPHABET[(b1 << 2) & 0x3F].to_s
      out << "="
    end
    out
  end

  # RFC 4648 §4 decode — inverse of strict_encode64. Skips "=" padding
  # and any non-alphabet byte. Accumulates 6 bits per input char into a
  # bit buffer, emitting one byte per 8 accumulated bits. Added for the
  # Action Cable glue's signed_stream_name decode (the encode side is
  # turbo_stream_from). Returns the decoded String.
  def self.strict_decode64(s)
    out = +""
    acc = 0
    nbits = 0
    i = 0
    n = s.length
    while i < n
      val = char_value(s[i])
      i = i + 1
      if val < 0
        next   # padding "=" or stray byte
      end
      acc = (acc << 6) | val
      nbits = nbits + 6
      if nbits >= 8
        nbits = nbits - 8
        byte = (acc >> nbits) & 0xFF
        out << byte.chr
      end
    end
    out
  end

  # Map one base64 alphabet character to its 6-bit value, or -1 for
  # anything outside the alphabet (padding / separators).
  def self.char_value(c)
    idx = 0
    while idx < 64
      if ALPHABET[idx] == c
        return idx
      end
      idx = idx + 1
    end
    -1
  end
end
