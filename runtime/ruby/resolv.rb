# Resolv — the one method of Ruby's resolver the corpus reaches, and it
# exists here for the same reason `ipaddr.rb` does: the ruby family has
# the stdlib's and the strict targets have nothing at all.
#
# THE RUBY FAMILY NEVER LOADS THIS FILE. `project::ruby_runtime_files`
# swaps it for a bare `require "resolv"` on the CRuby and JRuby trees,
# exactly as it does for `ipaddr` and `zlib`, and for the sharper of the
# two reasons ipaddr gives: something over there already loads the
# stdlib's `Resolv` (net/http reaches it), and a second definition beside
# it is a `TypeError: superclass mismatch` at REQUIRE time. It is also
# what makes the app's own tests work — campfire stubs
# `Resolv.getaddresses` with mocha, and a stub only lands on the class
# the caller actually dispatches to.
#
# ON A STRICT TARGET the lookup itself is `Resolv.resolve`, a hook this
# file defines as a loud failure and a target with a resolver REOPENS:
# `runtime/spinel/resolv_spinel.rb` answers it from `Socket.getaddrinfo`.
# The stub walk stays here, above the hook, so a stubbed host never
# reaches a socket on any target. Where nothing reopens it, the hook
# fails rather than answering: there is no resolver to bind to, and the
# two quiet alternatives are both worse — an empty list reads as "this
# host has no address", which `Surfguard.resolve_public_ips` reports as
# `Unresolvable` and a caller treats as a transient DNS miss, a silent
# no-op for every push delivery. `GemFacade.fail!` (rather than a bare
# `raise`) keeps the typed tail below statically live, which is what
# makes the return type inferable under AOT; see `gem_facades.rb`'s own
# note.
#
# `getaddresses`, not `getaddress`: it is what surfguard's policy is
# written against — every address a host answers with, honouring
# /etc/hosts, so what gets validated is what the connection layer will
# reach.
require_relative "gem_facades"

class Resolv
  # Raised by the stdlib when a lookup fails (NXDOMAIN, timeout).
  # `Surfguard.resolve` rescues it, and a rescue clause is EVALUATED
  # when an exception passes through it — so the constant has to exist
  # on every target, not just the ones that can resolve.
  class ResolvError < StandardError
  end

  # ---- Test stub slot ---------------------------------------------
  #
  # campfire's suite stubs this method thirteen times
  # (`Resolv.stubs(:getaddresses).with(host).returns([ip])`), and mocha
  # is a Ruby metaprogramming library with nothing for a strict target
  # to compile against. Rather than treat those tests as a permanent
  # ceiling, the facade carries the seam: a stub REPLACES the answer,
  # and with no stub installed the body fails loudly exactly as before.
  #
  # Parallel constant Arrays, not a Hash and not a module-level ivar —
  # the idiom `runtime/broadcasts.rb` establishes and explains: spinel
  # supports constants and array mutation, module-level instance
  # variables are less certain.
  #
  # SEEDED for the same reason `Broadcasts::TRANSPORTS` is. An
  # always-empty literal gives spinel nothing to infer the element type
  # from, and the whole table lands behind unresolved-call gates. `""`
  # is the seed because no real lookup asks for the empty host, so the
  # sentinel can never match.
  STUB_HOSTS = [ "" ]
  STUB_ADDRS = [ [ "" ] ]

  # The catch-all form, `stubs(:getaddresses).returns(v)` with no
  # `.with` — campfire's `stub_dns_resolution(*ips)` helper writes it,
  # and it answers for EVERY host rather than a keyed one.
  #
  # Two holders rather than one nullable, because "no default" and
  # "default is the empty list" are different answers and a single
  # Array cannot tell them apart. Single-element Array as a settable
  # holder is the `Broadcasts::TRANSPORTS` idiom; the value slot is
  # seeded for the element-type reason the note above gives, and the
  # flag is what actually says whether it counts.
  STUB_ANY = [ [ "" ] ]
  STUB_ANY_ON = [ false ]

  # `stubs(:getaddresses).raises(error)` — campfire's `stub_dns_failure`,
  # a host that resolves to nothing. The class travels as a value (it is
  # the helper's own parameter, `error = Resolv::ResolvError`) and is
  # raised as one. `raises` and the catch-all are both unconditional
  # matchers, so whichever was installed LAST shadows the other —
  # mocha consults the newest expectation first — and each install
  # turns the other off.
  STUB_RAISE = [ StandardError ]
  STUB_RAISE_ON = [ false ]

  # `stubs(:getaddresses).with { |*| … }.returns(addrs)` — a predicate
  # over the host, consulted before every other stub because it was
  # installed inside the test where the others came from setup.
  # campfire's counts resolver calls with it.
  STUB_WHERE = [ nil ]
  STUB_WHERE_ADDRS = [ [ "" ] ]

  # Install or REPLACE one host's answer. Replacement matters: a single
  # test re-stubs the same host with a second value
  # (`opengraph_location_test` maps `metadata.internal` to an IPv4-mapped
  # IPv6 address and then to its compressed spelling), so a
  # write-once slot would silently answer the first value twice.
  def self.stub_getaddresses(host, addrs)
    i = 0
    while i < STUB_HOSTS.length
      if STUB_HOSTS[i] == host
        STUB_ADDRS[i] = addrs
        return nil
      end
      i += 1
    end
    STUB_HOSTS << host
    STUB_ADDRS << addrs
    nil
  end

  # Install the catch-all answer. A later keyed stub still wins: the
  # host loop runs first.
  def self.stub_getaddresses_any(addrs)
    STUB_ANY[0] = addrs
    STUB_ANY_ON[0] = true
    STUB_RAISE_ON[0] = false
    nil
  end

  def self.stub_getaddresses_raises(error)
    STUB_RAISE[0] = error
    STUB_RAISE_ON[0] = true
    STUB_ANY_ON[0] = false
    nil
  end

  def self.stub_getaddresses_where(addrs, &blk)
    STUB_WHERE[0] = blk
    STUB_WHERE_ADDRS[0] = addrs
    nil
  end

  # Drop every installed stub. The emitted helper's teardown calls this,
  # so a stub cannot outlive the test that wrote it — the same leak
  # mocha's own `mocha_teardown` exists to prevent.
  def self.clear_getaddresses_stubs
    STUB_HOSTS.clear
    STUB_ADDRS.clear
    STUB_HOSTS << ""
    STUB_ADDRS << [ "" ]
    STUB_ANY_ON[0] = false
    STUB_RAISE_ON[0] = false
    STUB_WHERE[0] = nil
    nil
  end

  def self.getaddresses(host)
    pred = STUB_WHERE[0]
    return STUB_WHERE_ADDRS[0] if !pred.nil? && pred.call(host)
    raise STUB_RAISE[0] if STUB_RAISE_ON[0]
    i = 0
    while i < STUB_HOSTS.length
      return STUB_ADDRS[i] if STUB_HOSTS[i] == host
      i += 1
    end
    return STUB_ANY[0] if STUB_ANY_ON[0]
    resolve(host)
  end

  # The unstubbed lookup — the seam a target with a resolver reopens
  # (see the header). Every address the host answers with, in the
  # order the resolver gave them; an unknown host is `[]`, which is what
  # Ruby's own `getaddresses` answers too (it is `getaddress` that
  # raises).
  def self.resolve(host)
    GemFacade.fail!("Resolv.getaddresses")
    [ host ]
  end
end
