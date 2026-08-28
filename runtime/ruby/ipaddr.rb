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
# PREFIXES ARE HERE NOW, and they arrived the way the note below said
# they would. `RestrictedHTTP::PrivateNetworkGuard` maps ten CIDR
# strings through `IPAddr.new` in a CLASS BODY, so campfire's binary
# died at LOAD with `invalid address: 0.0.0.0/8` before it could serve
# anything. `@prefix` is the significant-bit count — 32 or 128 for a
# bare host address, which is what makes every predicate above
# prefix-agnostic — and the address is MASKED to it on the way in,
# because the stdlib does: `IPAddr.new("10.1.2.3/8").to_s` is
# "10.0.0.0" (measured, Ruby 4.0.5).
#
# WHAT IS STILL NOT HERE:
#
# * **The NETMASK spelling.** `IPAddr.new("1.2.3.4/255.255.0.0")` is
#   legal in the stdlib and raises here. Nothing in the corpus writes
#   it, and a prefix parser that quietly accepted a dotted quad as a
#   bit-count would be worse than one that says no.
# * **`to_i`.** For IPv6 it is a 128-bit integer, which is precisely
#   the representation this file exists to avoid (see above). campfire's
#   `PrivateNetworkGuard#embedded_ipv4` asks for one; that method also
#   needs `Array#pack`/`String#unpack`, so it is a gap either way and
#   `to_i` alone would not close it.
# * **A bad prefix raises `InvalidAddressError`**, where the stdlib
#   raises `IPAddr::InvalidPrefixError`. campfire rescues only the
#   former, so the divergence lands it in the "treat as private" branch
#   — the safe direction for a guard, and the one a caller that rescues
#   nothing would take anyway.
# * `to_range`, `succ`, arithmetic, and `to_s`'s compressed IPv6 form.
#   None is reached by the corpus.
class IPAddr
  # Raised by `IPAddr.new` on a string that is not an address, which is
  # the branch campfire's validation rescues.
  class InvalidAddressError < StandardError
  end

  def initialize(addr)
    text = addr
    prefix = -1
    idx = addr.index("/")
    if idx
      text = addr[0, idx]
      prefix = IPAddr.parse_prefix(addr[idx + 1, addr.length - idx - 1], addr)
    end
    octets = IPAddr.parse_octets(text)
    bits = octets.length * 8
    raise InvalidAddressError, "invalid address: #{addr}" if prefix > bits
    prefix = bits if prefix < 0
    @prefix = prefix
    @octets = IPAddr.mask(octets, prefix)
  end

  # The parsed address, most-significant octet first: 4 entries for
  # IPv4, 16 for IPv6. Already masked to `prefix`.
  def octets
    @octets
  end

  # Significant bits: 32 or 128 for a bare host address, the `/N` for a
  # network. Same default the stdlib reports.
  def prefix
    @prefix
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

  # `::a.b.c.d` — the DEPRECATED IPv4-compatible form, which is not the
  # mapped one above: octets 10 and 11 are zero rather than 0xff.
  # Rails' guard asks for it beside `ipv4_mapped?` and treats either as
  # private. `::` and `::1` are excluded, matching the stdlib's own
  # `@addr < 2` test (measured: `::0.0.0.1` is false, `::1.2.3.4` is
  # true).
  def ipv4_compat?
    return false if !ipv6?
    i = 0
    while i < 12
      return false if @octets[i] != 0
      i += 1
    end
    value = ((@octets[12] * 256 + @octets[13]) * 256 + @octets[14]) * 256 + @octets[15]
    value >= 2
  end

  # Is `other` inside this network? THE FAMILIES MUST MATCH — a v4
  # network does not include a v6 address, not even the mapped form of
  # one (measured: `10.0.0.0/8` does not include `::ffff:10.1.2.3`).
  # `@octets` is already masked, so this masks the other side to the
  # same prefix and compares.
  def include?(other)
    o = other.octets
    return false if o.length != @octets.length
    masked = IPAddr.mask(o, @prefix)
    i = 0
    while i < @octets.length
      return false if masked[i] != @octets[i]
      i += 1
    end
    true
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

  # The `/N` of a CIDR string. Decimal digits only: the stdlib's
  # dotted-quad netmask spelling is declined here rather than
  # approximated (see the header). An empty or non-numeric prefix is an
  # invalid address, which is what the stdlib answers for `"1.2.3.4/"`
  # too.
  def self.parse_prefix(part, addr)
    raise InvalidAddressError, "invalid address: #{addr}" if part.length == 0
    raise InvalidAddressError, "invalid address: #{addr}" if part.length > 3
    value = 0
    i = 0
    while i < part.length
      d = digit_value(part[i, 1], 10)
      raise InvalidAddressError, "invalid address: #{addr}" if d < 0
      value = value * 10 + d
      i += 1
    end
    value
  end

  # `octets` with everything below `prefix` bits cleared. The stdlib
  # masks on construction — `IPAddr.new("10.1.2.3/8").to_s` is
  # "10.0.0.0" — so every comparison downstream is against a canonical
  # network address and `include?` needs no special case for a caller
  # that wrote host bits into a CIDR.
  def self.mask(octets, prefix)
    out = []
    i = 0
    while i < octets.length
      bit = i * 8
      if bit + 8 <= prefix
        out << octets[i]
      elsif bit >= prefix
        out << 0
      else
        out << (octets[i] & ((0xff << (8 - (prefix - bit))) & 0xff))
      end
      i += 1
    end
    out
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
