module Tep
  VERSION = "0.8.1-vendored"

  def self.str_hash
    # Missing-key reads must return "" — the tep readers assume it (parser.rb
    # cookie handling, request.rb Connection/Content-Type, etc.).
    Hash.new("")
  end

  # Holder for a Fiber so the cooperative scheduler can keep them in
  # a typed array. Spinel's `[Fiber.new { ... }]` array literal infers
  # IntArray (Fiber is a built-in pointer type, not a user class spinel
  # tracks via PtrArray), so a one-attribute wrapper class is the
  # cheapest way to put them in a homogeneous container. Vendored from
  # tep's lib/tep.rb (Tep::FiberSlot).
  class FiberSlot
    attr_accessor :f
    def initialize(f)
      @f = f
    end
  end

  # A canonical no-op fiber body, used to type-seed Fiber-bearing
  # collections without running anything user-visible.
  def self.seed_fiber_noop
    0
  end

  # Shutdown hook. Tep::Server::Scheduled calls Tep.on_shutdown after
  # the accept loop breaks on SIGTERM/SIGINT. Upstream tep fans this
  # out to run_end / Events hooks; roundhouse has none, so it's a
  # no-op (defined so the call resolves rather than emitting 0).
  def self.on_shutdown
    0
  end

  # str_find -- naive substring search returning the int position of
  # `needle` in `s` starting from `start`, or -1 if not found. Callers
  # use `if x < 0` int comparison, which can't narrow against the
  # int|nil that String#index returns under spinel's narrowing model.
  # Vendored from tep's lib/tep.rb (Tep.str_find).
  def self.str_find(s, needle, start)
    nlen = needle.length
    slen = s.length
    pos = start
    while pos <= slen - nlen
      if s[pos, nlen] == needle
        return pos
      end
      pos += 1
    end
    -1
  end
end
