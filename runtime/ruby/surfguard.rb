# Surfguard — the SSRF address policy campfire's `RestrictedHTTP` used to
# carry inline and now takes from a gem (basecamp/surfguard, MIT).
#
# PORTED, NOT FAÇADED, and the reason is the Gemfile: surfguard is a GIT
# gem (`gem "surfguard", github: "basecamp/surfguard"`), so the
# `RUNTIME_GEMS` route — declare it and let the ruby tree require the
# real one — has nothing to install. The gem is pure Ruby over `IPAddr`
# and `Resolv`, and `runtime/ruby/ipaddr.rb` is already the port of the
# first, so this is the same arrangement one level up: one
# implementation, every target, ours.
#
# THE RULE TABLE IS THE GEM'S, VERBATIM (revision 910be917). Every CIDR,
# every comment marker, and the ORDER of the branches in
# `blocked_ipaddr?` are copied rather than re-derived — an address
# wrongly called public is a fetch that should not have happened, which
# is the inflector mistake in a much more expensive place. The three
# decisions the gem's header calls out are kept as they are:
#
#   * `Resolv.getaddresses`, not `getaddress` — every address the host
#     answers with, honouring /etc/hosts, so what is validated is what
#     the connection layer will reach.
#   * The RFC 8215 local-use NAT64 block (`64:ff9b:1::/48`) is refused
#     WHOLE, not decoded: a Pref64 inside it can be /32…/96 and the
#     embedded position is not recoverable from the address (RFC 6052
#     §2.2), so reading the low 32 bits reads the wrong octets.
#   * SIIT's `::ffff:0:0:0/96` is NOT the familiar IPv4-mapped
#     `::ffff:0:0/96`, they do not overlap, and Ruby has no predicate
#     for it — `ipv4_mapped?`, `ipv4_compat?`, `private?`, `loopback?`
#     and `link_local?` are all false for `::ffff:0:169.254.169.254`.
#
# THE REPRESENTATION IS OCTETS, because `IPAddr`'s is. The gem's
# `embedded_ipv4` masks `to_i` with 0xffffffff; `to_i` is the 128-bit
# integer the port exists to avoid (see runtime/ruby/ipaddr.rb), and the
# low 32 bits ARE the last four octets, so it reads them directly.
#
# WHAT IS NOT HERE: `resolvable_public_ip?`, `enforce_public_ip` and
# `resolve_public_ip`, the three URL-taking entry points. All three are
# `host_of(url)` plus a policy already below, and nothing in the corpus
# calls one — campfire reaches `resolve_public_ips` and
# `blocked_address?` only. Add them beside their siblings when an app
# asks, not before.
#
# `blocked_address?` takes a STRING where the gem takes "an IPAddr or
# anything IPAddr.new understands". The internal `blocked_ipaddr?` is
# the IPAddr arm, so neither one is polymorphic — the same split every
# other runtime file makes for the strict targets.
require_relative "ipaddr"

module Surfguard
  # A host that resolves to an address we refuse.
  class Violation < StandardError
  end

  # The host answered with no address at all. Deliberately NOT a
  # Violation: nothing was refused, the lookup came back empty, which is
  # usually transient. campfire's `Push::Subscription` rescues both and
  # treats them alike; a caller that retires an endpoint on a Violation
  # must not retire it on one bad DNS minute.
  class Unresolvable < StandardError
  end

  # IPv4 special-use ranges that must never be a fetch target (RFC
  # 5735/6890, plus CGNAT and benchmarking). RFC 1918 / loopback /
  # link-local are also covered by the IPAddr predicates in
  # `disallowed_ipv4?`; they are restated so the policy is complete and
  # auditable in one place.
  DISALLOWED_IPV4 = [
    IPAddr.new("0.0.0.0/8"),        # "This" network (RFC 1122)
    IPAddr.new("10.0.0.0/8"),       # Private (RFC 1918)
    IPAddr.new("100.64.0.0/10"),    # Carrier-grade NAT (RFC 6598)
    IPAddr.new("127.0.0.0/8"),      # Loopback (RFC 1122)
    IPAddr.new("169.254.0.0/16"),   # Link-local (RFC 3927) — the cloud metadata endpoint
    IPAddr.new("172.16.0.0/12"),    # Private (RFC 1918)
    IPAddr.new("192.0.0.0/24"),     # IETF protocol assignments (RFC 6890)
    IPAddr.new("192.0.2.0/24"),     # TEST-NET-1 (RFC 5737)
    IPAddr.new("192.88.99.0/24"),   # 6to4 relay anycast (RFC 7526)
    IPAddr.new("192.168.0.0/16"),   # Private (RFC 1918)
    IPAddr.new("198.18.0.0/15"),    # Benchmark testing (RFC 2544)
    IPAddr.new("198.51.100.0/24"),  # TEST-NET-2 (RFC 5737)
    IPAddr.new("203.0.113.0/24"),   # TEST-NET-3 (RFC 5737)
    IPAddr.new("224.0.0.0/4"),      # Multicast (RFC 5771)
    IPAddr.new("240.0.0.0/4")       # Reserved / future use (RFC 1112)
  ].freeze

  # IPv6 special-use ranges beyond what `private?` (ULA fc00::/7,
  # including the IMDSv6 address fd00:ec2::254), `loopback?` (::1) and
  # `link_local?` (fe80::/10) already cover. 6to4 and Teredo are
  # deprecated transition mechanisms with no legitimate fetch target —
  # 2002:7f00:1:: is just a 6to4 spelling of 127.0.0.1.
  DISALLOWED_IPV6 = [
    IPAddr.new("::/128"),           # Unspecified (RFC 4291)
    IPAddr.new("100::/64"),         # Discard-only (RFC 6666)
    IPAddr.new("2001::/32"),        # Teredo (RFC 4380)
    IPAddr.new("2001:2::/48"),      # Benchmark testing (RFC 5180)
    IPAddr.new("2001:db8::/32"),    # Documentation (RFC 3849)
    IPAddr.new("2002::/16"),        # 6to4 (RFC 3056)
    IPAddr.new("fec0::/10"),        # Deprecated site-local (RFC 3879)
    IPAddr.new("ff00::/8")          # Multicast (RFC 4291)
  ].freeze

  # NAT64 embeds an IPv4 target that must be re-checked as IPv4. The
  # well-known prefix is a fixed /96, so the embedded octets are always
  # the low four.
  NAT64_WELL_KNOWN = IPAddr.new("64:ff9b::/96")    # RFC 6052

  # Refused whole, not decoded — see the header.
  NAT64_LOCAL_USE = IPAddr.new("64:ff9b:1::/48")   # RFC 8215

  # SIIT IPv4-translated. A fixed /96 like the well-known prefix, so the
  # low four octets are the embedded address.
  IPV4_TRANSLATABLE = IPAddr.new("::ffff:0:0:0/96") # RFC 2765

  # Every PUBLIC address the host resolves to, IPv4 ahead of IPv6, DNS
  # order preserved within each family so a provider's round-robin still
  # spreads load. Empty when the host resolves but every address is
  # blocked; raises `Unresolvable` when it resolves to nothing at all,
  # so the caller can tell a refusal from a lookup failure.
  def self.resolve_public_ips(host)
    addresses = resolve(host)
    raise Unresolvable.new("No address for #{host}") if addresses.empty?

    out = []
    addresses.each do |ip|
      out << ip.to_s if ip.ipv4? && !blocked_ipaddr?(ip)
    end
    addresses.each do |ip|
      out << ip.to_s if !ip.ipv4? && !blocked_ipaddr?(ip)
    end
    out
  end

  # The classification core, over the spelling a caller has: a String.
  # Errs closed — an address it cannot parse is blocked.
  def self.blocked_address?(ip)
    begin
      blocked_ipaddr?(IPAddr.new(ip))
    rescue IPAddr::InvalidAddressError
      true
    end
  end

  # The same question over a parsed address. Branch order is the gem's:
  # the two embedded forms DNS never legitimately returns are refused
  # first, whatever they wrap.
  def self.blocked_ipaddr?(ipaddr)
    if ipaddr.ipv4_mapped? || ipaddr.ipv4_compat?
      true
    elsif ipaddr.ipv4?
      disallowed_ipv4?(ipaddr)
    elsif NAT64_LOCAL_USE.include?(ipaddr)
      true
    elsif NAT64_WELL_KNOWN.include?(ipaddr) || IPV4_TRANSLATABLE.include?(ipaddr)
      disallowed_ipv4?(embedded_ipv4(ipaddr))
    else
      disallowed_ipv6?(ipaddr)
    end
  end

  # An IP-literal host skips DNS, so a public literal URL resolves
  # directly and an internal literal is still caught by the policy.
  # Otherwise `Resolv.getaddresses` — the Hosts+DNS chain, matching what
  # the connection layer will use.
  def self.resolve(host)
    literal = ip_literal(host)
    return [ literal ] unless literal.nil?

    begin
      out = []
      Resolv.getaddresses(host).each { |a| out << IPAddr.new(a) }
      out
    rescue Resolv::ResolvError
      []
    end
  end

  # nil for anything that is not already an address. Kept separate so
  # the DNS call above sits in a method body where its own rescue can
  # reach it.
  def self.ip_literal(host)
    begin
      IPAddr.new(host)
    rescue IPAddr::InvalidAddressError
      nil
    end
  end

  def self.disallowed_ipv4?(ipaddr)
    ipaddr.private? || ipaddr.loopback? || ipaddr.link_local? ||
      DISALLOWED_IPV4.any? { |range| range.include?(ipaddr) }
  end

  def self.disallowed_ipv6?(ipaddr)
    ipaddr.private? || ipaddr.loopback? || ipaddr.link_local? ||
      DISALLOWED_IPV6.any? { |range| range.include?(ipaddr) }
  end

  # RFC 6052 §2.2: a fixed /96 translation prefix carries the IPv4
  # target in the low 32 bits — the last four octets.
  def self.embedded_ipv4(ipaddr)
    o = ipaddr.octets
    IPAddr.new("#{o[12]}.#{o[13]}.#{o[14]}.#{o[15]}")
  end
end
