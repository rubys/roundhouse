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
# ON A STRICT TARGET the body fails loudly rather than answering. There
# is no resolver to bind to, and the two quiet alternatives are both
# worse: an empty list reads as "this host has no address", which
# `Surfguard.resolve_public_ips` reports as `Unresolvable` and a caller
# treats as a transient DNS miss — a silent no-op for every push
# delivery. `GemFacade.fail!` (rather than a bare `raise`) keeps the
# typed tail below statically live, which is what makes the return type
# inferable under AOT; see `gem_facades.rb`'s own note.
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

  def self.getaddresses(host)
    GemFacade.fail!("Resolv.getaddresses")
    [ host ]
  end
end
