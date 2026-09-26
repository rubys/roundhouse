# `typeid` 0.2.2 (with the `uuid7` 0.2.0 generator it requires), ported:
# `TypeID.new(prefix)`, the one call the corpus makes. lobsters' Token
# concern mints every record's public token with it —
#
#     self.token ||= TypeID.new(self.class.to_s.parameterize)
#
# — so a target without the gem raised NameError on the first record it
# built, which on spinel was the login POST.
#
# A TypeID is `<prefix>_<suffix>`: the suffix is a UUIDv7 (48-bit
# millisecond timestamp, version nibble 7, variant bits 10, 74 random
# bits) written as 26 characters of Crockford base32, lowercase, so the
# first character is always 0-7. `user_01kxshz4b3fr398b04cs8rvqjz` is a
# real one from lobsters' dev database.
#
# ONE DIFFERENCE, AND IT IS THE RUNTIME'S RULE, NOT A SHORTCUT. The gem's
# `TypeID < String` (and `TypeID::UUID < String`); a subclass of a
# builtin is a shape the strict targets do not carry (see the
# user_agent port's header for the same rule). So `TypeID` here is a
# module and `TypeID.new` answers a plain String with the same text. The
# only consumer stores it in a String column, where the gem's object
# would have been written as that same text.
#
# NOT HERE: `from_string` / `from_uuid` / `#uuid` / `#prefix` /
# `#suffix`, `TypeID.nil`, the `timestamp:` / `suffix:` keywords. Nothing
# in the corpus calls them.
#
# The ruby family does not load this file: `project::ruby_family_runtime
# _files` swaps it for `require "typeid"`, so CRuby and JRuby run the gem
# itself. The suite beside this file checks the port's encoding against
# the gem's on fixed bytes.
module TypeID
  MAX_PREFIX_LENGTH = 63

  # The gem's `TypeID::Error` is `StandardError` too.
  class Error < StandardError
  end

  def self.new(prefix)
    TypeID.validate_prefix(prefix)
    suffix = TypeID.encode(TypeID.uuid7_bytes(TypeID.timestamp_ms, SecureRandom.hex(10)))
    return suffix if prefix.empty?
    prefix + "_" + suffix
  end

  # The gem's prefix rules, in its order and with its messages.
  def self.validate_prefix(prefix)
    if prefix.length > TypeID::MAX_PREFIX_LENGTH
      raise Error, "prefix length cannot be greater than " + TypeID::MAX_PREFIX_LENGTH.to_s
    end
    i = 0
    while i < prefix.length
      ch = prefix[i]
      unless ch == "_" || (ch >= "a" && ch <= "z")
        raise Error, "prefix must be lowercase ASCII characters"
      end
      i += 1
    end
    if prefix.start_with?("_") || prefix.end_with?("_")
      raise Error, "prefix cannot start or end with an underscore"
    end
    nil
  end

  # Milliseconds since the Unix epoch — the gem's
  # `Process.clock_gettime(Process::CLOCK_REALTIME, :millisecond)`.
  def self.timestamp_ms
    t = Time.now
    t.to_i * 1000 + t.usec / 1000
  end

  # uuid7's generator: 48 bits of timestamp, then `rand_a` (12 bits)
  # under version 7, then `rand_b` (62 bits) under the RFC 4122 variant.
  # `random_hex` is 20 hex digits — the 10 random bytes the gem reads
  # with `SecureRandom.gen_random(10)`. Taken as hex because the ported
  # String every target shares answers code points, not bytes (see the
  # zlib port's header).
  def self.uuid7_bytes(timestamp, random_hex)
    ts = timestamp & 0xffffffffffff
    rnd = []
    i = 0
    while i < 10
      rnd << TypeID.hex_digit(random_hex[2 * i]) * 16 + TypeID.hex_digit(random_hex[2 * i + 1])
      i += 1
    end
    bytes = []
    shift = 40
    while shift >= 0
      bytes << ((ts >> shift) & 0xff)
      shift -= 8
    end
    bytes << (0x70 | (rnd[0] & 0x0f))
    bytes << rnd[1]
    bytes << (0x80 | (rnd[2] & 0x3f))
    j = 3
    while j < 10
      bytes << rnd[j]
      j += 1
    end
    bytes
  end

  def self.hex_digit(ch)
    return ch.ord - 48 if ch >= "0" && ch <= "9"
    return ch.ord - 87 if ch >= "a" && ch <= "f"
    return ch.ord - 55 if ch >= "A" && ch <= "F"
    raise Error, "invalid hex digit"
  end

  # The gem's `Base32::ALPHABET` — Crockford's, lowercase. A method, not
  # a constant: the runtime's typing reads a module-level String
  # constant as a class reference.
  def self.alphabet
    "0123456789abcdefghjkmnpqrstvwxyz"
  end

  # The gem's `TypeID::UUID::Base32.encode`: 16 bytes (128 bits) as 26
  # five-bit groups, the first group holding only the top 3 bits. Read as
  # one 130-bit big-endian number padded with two zero bits on the left,
  # which is what the gem's hand-unrolled masks compute.
  def self.encode(bytes)
    raise Error, "invalid bytes size" unless bytes.length == 16
    alphabet = TypeID.alphabet
    out = ""
    acc = 0
    bits = 2
    i = 0
    while i < 16
      acc = (acc << 8) | bytes[i]
      bits += 8
      while bits >= 5
        bits -= 5
        out = out + alphabet[(acc >> bits) & 31]
      end
      acc = acc & ((1 << bits) - 1)
      i += 1
    end
    out
  end
end
