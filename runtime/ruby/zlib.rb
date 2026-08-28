# Zlib.crc32 — the one entry point the corpus reaches, computed here
# rather than bound to a C library.
#
# campfire picks an avatar's colour with
#
#     AVATAR_COLORS[Zlib.crc32(user.to_param) % AVATAR_COLORS.size]
#
# so the checksum is not an internal detail: it decides a pixel colour
# in the rendered page, and any answer but zlib's exact one shows up as
# a wrong avatar in the compare oracle. That is why this is a port and
# not an approximation.
#
# THE ALGORITHM IS CRC-32/ISO-HDLC (the one zlib, PNG and gzip use),
# written out rather than table-driven: reflected input and output,
# polynomial 0xEDB88320 (the bit-reversed 0x04C11DB7), initial and
# final value 0xFFFFFFFF. A 256-entry lookup table would be four times
# faster and would need module-level mutable state at load time, which
# is the one shape the strict targets have no home for; the corpus
# input is a record id, so eight shifts per byte is nothing.
#
# THE BYTES ARE DERIVED, NOT READ. A CRC is defined over BYTES, and the
# ported string subset every target shares has `[]` and `ord` — which
# answer CODE POINTS — but no `getbyte` or `bytes`. So each code point
# is expanded to its UTF-8 encoding here (`utf8_bytes`), which is what
# Ruby's own String would have handed zlib for the same source text.
# For ASCII the two coincide; above U+007F they do not, and skipping
# the expansion would have made this right for record ids and silently
# wrong for anything else.
#
# NOT HERE: the second `crc` argument (`Zlib.crc32(str, prior)`, for
# checksumming a stream in chunks), `crc32_combine`, `adler32`, and
# every compression entry point. Deflate needs a real implementation,
# not a port, and nothing in the corpus asks for one.
module Zlib
  # Ruby's `Zlib.crc32(string)` — the CRC-32 of `string`'s bytes, as an
  # unsigned 32-bit Integer.
  def self.crc32(str)
    crc = 0xFFFFFFFF
    i = 0
    len = str.length
    while i < len
      bytes = Zlib.utf8_bytes(str[i].ord)
      b = 0
      while b < bytes.length
        crc = crc ^ bytes[b]
        j = 0
        while j < 8
          crc = if (crc & 1) == 1
            (crc >> 1) ^ 0xEDB88320
          else
            crc >> 1
          end
          j += 1
        end
        b += 1
      end
      i += 1
    end
    crc ^ 0xFFFFFFFF
  end

  # One code point's UTF-8 encoding, most significant byte first. The
  # four-range split is UTF-8's own definition; a lone surrogate or a
  # value above U+10FFFF cannot come out of `String#ord`, so there is
  # no error case to answer.
  def self.utf8_bytes(cp)
    if cp < 0x80
      [ cp ]
    elsif cp < 0x800
      [ 0xC0 | (cp >> 6), 0x80 | (cp & 0x3F) ]
    elsif cp < 0x10000
      [ 0xE0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3F), 0x80 | (cp & 0x3F) ]
    else
      [ 0xF0 | (cp >> 18), 0x80 | ((cp >> 12) & 0x3F),
        0x80 | ((cp >> 6) & 0x3F), 0x80 | (cp & 0x3F) ]
    end
  end
end
