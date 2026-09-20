# `Digest::MD5`, for the compiled family — hexdigest, digest and
# base64digest, RFC 1321 in Ruby.
#
# Active Storage's direct-upload protocol is MD5: the browser declares
# a blob's checksum as the base64 MD5 of its bytes, the metadata POST
# signs it into the upload token, and the disk service holds the
# uploaded bytes to it (`runtime/spinel/active_storage_disk.rb`,
# `DiskController#update`). spinel's bundled `digest` package binds the
# runtime's SHA-256 and SHA-1 and nothing else (packages/digest/
# digest.rb), so the class is ported here for the spinel tree; the
# CRuby overlay has the stdlib's and never loads this file. Nothing in
# it is hot — one digest per upload.
#
# 32-bit arithmetic on a 64-bit Integer, masked at every add; the
# rotate is two shifts. `getbyte` rather than `unpack`, which the
# compiled family's String does not carry. `Base64` is the tree's own
# shim (runtime/base64.rb), loaded just before this file by boot.rb.
module Digest
  module MD5
    S = [7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
         5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
         4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
         6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21]
    K = [0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
         0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
         0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
         0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
         0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
         0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
         0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
         0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391]

    def self.rotl(x, c)
      ((x << c) | (x >> (32 - c))) & 0xFFFFFFFF
    end

    # The 16 little-endian words of the 64-byte block at `at`.
    def self.words(data, at)
      w = []
      i = 0
      while i < 16
        o = at + i * 4
        w << (data.getbyte(o).to_i | (data.getbyte(o + 1).to_i << 8) |
              (data.getbyte(o + 2).to_i << 16) | (data.getbyte(o + 3).to_i << 24))
        i += 1
      end
      w
    end

    # The raw 16 digest bytes.
    def self.digest(message)
      n = message.bytesize
      # Padding: 0x80, zeros to 56 mod 64, then the bit length as a
      # little-endian 64-bit word.
      padded = message.b + "\x80".b
      padded << "\x00".b while padded.bytesize % 64 != 56
      bits = n * 8
      i = 0
      while i < 8
        padded << ((bits >> (i * 8)) & 0xFF).chr.b
        i += 1
      end
      a0 = 0x67452301
      b0 = 0xefcdab89
      c0 = 0x98badcfe
      d0 = 0x10325476
      at = 0
      while at < padded.bytesize
        m = words(padded, at)
        a = a0
        b = b0
        c = c0
        d = d0
        j = 0
        while j < 64
          if j < 16
            f = (b & c) | ((~b) & d)
            g = j
          elsif j < 32
            f = (d & b) | ((~d) & c)
            g = (5 * j + 1) % 16
          elsif j < 48
            f = b ^ c ^ d
            g = (3 * j + 5) % 16
          else
            f = c ^ (b | ((~d) & 0xFFFFFFFF))
            g = (7 * j) % 16
          end
          f = (f + a + K[j] + m[g]) & 0xFFFFFFFF
          a = d
          d = c
          c = b
          b = (b + rotl(f, S[j])) & 0xFFFFFFFF
          j += 1
        end
        a0 = (a0 + a) & 0xFFFFFFFF
        b0 = (b0 + b) & 0xFFFFFFFF
        c0 = (c0 + c) & 0xFFFFFFFF
        d0 = (d0 + d) & 0xFFFFFFFF
        at += 64
      end
      out = +"".b
      [a0, b0, c0, d0].each do |v|
        out << (v & 0xFF).chr.b
        out << ((v >> 8) & 0xFF).chr.b
        out << ((v >> 16) & 0xFF).chr.b
        out << ((v >> 24) & 0xFF).chr.b
      end
      out
    end

    def self.hexdigest(message)
      hex = "0123456789abcdef"
      out = +""
      digest(message).each_byte do |byte|
        out << hex[byte >> 4].to_s
        out << hex[byte & 0xF].to_s
      end
      out
    end

    def self.base64digest(message)
      Base64.strict_encode64(digest(message))
    end
  end
end
