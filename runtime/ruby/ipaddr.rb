# IPAddr — the slice of Ruby's stdlib class the corpus reaches, ported
# rather than derived.
#
# campfire validates that a ban's IP is public:
#
#     ip = IPAddr.new(ip_address)
#     if ip.loopback? || ip.private? || ip.link_local?
#     rescue IPAddr::InvalidAddressError
#
# THE RULE TABLE IS ipaddr.rb's, VERBATIM. Every prefix below is copied
# from the stdlib's own comments and masks (Ruby 4.0.5,
# lib/ruby/4.0.0/ipaddr.rb): 127.0.0.0/8; 10.0.0.0/8, 172.16.0.0/12 and
# 192.168.0.0/16 (RFC 1918) plus fc00::/7 (RFC 4193); 169.254.0.0/16
# (RFC 3927) plus fe80::/10 (RFC 4291); and the IPv4-mapped
# (`::ffff:a.b.c.d`) form of each, which the stdlib folds into the same
# three answers. Guessing at these would be the inflector mistake in a
# more expensive place — an address wrongly called public is a ban that
# does not take.
#
# THE REPRESENTATION IS NOT the stdlib's. Ruby keeps one Integer, which
# for IPv6 is 128 bits wide; the strict targets have no such integer and
# a bignum in this path would cost nine runtimes a primitive to answer
# three predicates. Octets answer them exactly as well — every rule in
# the table is a prefix test — and an Array of small Integers is a shape
# every target already has. `@octets` is 4 long for IPv4 and 16 for
# IPv6, so `ipv4?` is a length test.
#
# WHAT IS NOT HERE: prefix/netmask parsing (`IPAddr.new("10.0.0.0/8")`),
# `include?`, `to_range`, `succ`, arithmetic, and `to_s`'s compressed
# IPv6 form. None is reached by the corpus, and a partial one that
# silently answered would be worse than its absence.
class IPAddr
  # Raised by `IPAddr.new` on a string that is not an address, which is
  # the branch campfire's validation rescues.
  class InvalidAddressError < StandardError
  end

  def initialize(addr)
    @octets = IPAddr.parse_octets(addr)
  end

  # The parsed address, most-significant octet first: 4 entries for
  # IPv4, 16 for IPv6.
  def octets
    @octets
  end

  def ipv4?
    @octets.length == 4
  end

  def ipv6?
    @octets.length == 16
  end

  # `::ffff:a.b.c.d` — an IPv4 address carried in an IPv6 one. The
  # stdlib answers every predicate below about the embedded address, so
  # the tests here reduce to the v4 ones on the last four octets.
  def ipv4_mapped?
    return false if !ipv6?
    i = 0
    while i < 10
      return false if @octets[i] != 0
      i += 1
    end
    @octets[10] == 0xff && @octets[11] == 0xff
  end

  # 127.0.0.0/8, ::1, and the mapped form of the first.
  def loopback?
    return @octets[0] == 127 if ipv4?
    return v4_loopback?(12) if ipv4_mapped?
    i = 0
    while i < 15
      return false if @octets[i] != 0
      i += 1
    end
    @octets[15] == 1
  end

  # 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, fc00::/7, and the mapped
  # forms of the first three.
  def private?
    return v4_private?(0) if ipv4?
    return v4_private?(12) if ipv4_mapped?
    (@octets[0] & 0xfe) == 0xfc
  end

  # 169.254.0.0/16, fe80::/10, and the mapped form of the first.
  def link_local?
    return v4_link_local?(0) if ipv4?
    return v4_link_local?(12) if ipv4_mapped?
    @octets[0] == 0xfe && (@octets[1] & 0xc0) == 0x80
  end

  def to_s
    return "#{@octets[0]}.#{@octets[1]}.#{@octets[2]}.#{@octets[3]}" if ipv4?
    parts = []
    i = 0
    while i < 16
      parts << IPAddr.hex4(@octets[i] * 256 + @octets[i + 1])
      i += 2
    end
    parts.join(":")
  end

  # `IPAddr.new(str)` accepts either family; which one it is decided by
  # the first `:`, exactly as the stdlib's own dispatch does.
  # The parameter is a plain `String`, not `String?`. A nilable one
  # would need `addr.nil?` here, which is only typeable where the RBS
  # travels with the file, and the corpus caller reads a NOT NULL
  # column. A nil at the call site is a type error there, which is
  # where it belongs.
  def self.parse_octets(addr)
    raise InvalidAddressError, "invalid address: #{addr}" if addr.length == 0
    return parse_v6(addr) if addr.include?(":")
    parse_v4(addr)
  end

  def self.parse_v4(text)
    parts = text.split(".", -1)
    raise InvalidAddressError, "invalid address: #{text}" if parts.length != 4
    out = []
    i = 0
    while i < 4
      out << parse_dec_octet(parts[i], text)
      i += 1
    end
    out
  end

  def self.parse_dec_octet(part, text)
    raise InvalidAddressError, "invalid address: #{text}" if part.length == 0
    raise InvalidAddressError, "invalid address: #{text}" if part.length > 3
    value = 0
    i = 0
    while i < part.length
      d = digit_value(part[i, 1], 10)
      raise InvalidAddressError, "invalid address: #{text}" if d < 0
      value = value * 10 + d
      i += 1
    end
    raise InvalidAddressError, "invalid address: #{text}" if value > 255
    value
  end

  # IPv6, including the one elision `::` and the trailing dotted-quad
  # form the mapped addresses use.
  def self.parse_v6(text)
    head = text
    tail = ""
    elided = false
    idx = text.index("::")
    if idx
      elided = true
      head = text[0, idx]
      tail = text[idx + 2, text.length - idx - 2]
      # A second `::` is illegal, and the tail is where it would be.
      raise InvalidAddressError, "invalid address: #{text}" if tail.include?("::")
    end

    left = parse_v6_groups(head, text)
    right = parse_v6_groups(tail, text)
    fill = 16 - left.length - right.length
    if elided
      raise InvalidAddressError, "invalid address: #{text}" if fill < 0
    else
      raise InvalidAddressError, "invalid address: #{text}" if fill != 0
    end

    out = []
    out.concat(left)
    i = 0
    while i < fill
      out << 0
      i += 1
    end
    out.concat(right)
    out
  end

  # One side of a `::`, as OCTETS. Returns an empty list for an empty
  # side, which is what makes `::1` and `fe80::` both work.
  def self.parse_v6_groups(part, text)
    out = []
    return out if part.length == 0
    groups = part.split(":", -1)
    i = 0
    while i < groups.length
      group = groups[i]
      if group.include?(".")
        # A trailing dotted quad is only legal as the last group.
        raise InvalidAddressError, "invalid address: #{text}" if i != groups.length - 1
        out.concat(parse_v4(group))
      else
        out.concat(parse_hex_group(group, text))
      end
      i += 1
    end
    out
  end

  def self.parse_hex_group(group, text)
    raise InvalidAddressError, "invalid address: #{text}" if group.length == 0
    raise InvalidAddressError, "invalid address: #{text}" if group.length > 4
    value = 0
    i = 0
    while i < group.length
      d = digit_value(group[i, 1], 16)
      raise InvalidAddressError, "invalid address: #{text}" if d < 0
      value = value * 16 + d
      i += 1
    end
    [value / 256, value % 256]
  end

  # -1 for anything that is not a digit in the given base. Written out
  # rather than `Integer(str, base)` so no target needs a raising
  # numeric parse in this path.
  def self.digit_value(ch, base)
    n = -1
    n = ch.ord - 48 if ch >= "0" && ch <= "9"
    if base == 16
      n = ch.ord - 87 if ch >= "a" && ch <= "f"
      n = ch.ord - 55 if ch >= "A" && ch <= "F"
    end
    return -1 if n < 0 || n >= base
    n
  end

  def self.hex4(value)
    digits = "0123456789abcdef"
    out = +""
    shift = 12
    while shift >= 0
      out = out + digits[(value >> shift) & 0xf, 1]
      shift -= 4
    end
    out
  end

  private
    # The IPv4 rules, applied at `at` so the mapped form reuses them.
    def v4_loopback?(at)
      @octets[at] == 127
    end

    def v4_private?(at)
      return true if @octets[at] == 10
      return true if @octets[at] == 172 && (@octets[at + 1] & 0xf0) == 16
      @octets[at] == 192 && @octets[at + 1] == 168
    end

    def v4_link_local?(at)
      @octets[at] == 169 && @octets[at + 1] == 254
    end
end
