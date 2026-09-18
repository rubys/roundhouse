# TcpSocketStub — the expectation table `TCPSocket.expects(:open)` lowers
# to (`lower::mocha`'s row for it), and the seam the client consults
# before it connects.
#
# WHAT THE CORPUS ASKS FOR. campfire's DNS-rebinding tests prove that
# `Opengraph::Fetch` connects to the address it resolved, never to the
# hostname again, by putting two expectations on the socket:
#
#   TCPSocket.expects(:open).with { |*args, **| args.first == host }.never
#   TCPSocket.expects(:open).with { |*args, **| args.first == "1.2.3.4" && args[1] == 443 }
#     .throws(:dns_not_rebound)
#
# and asserting the throw. So a row is a PREDICATE over `(host, port)`
# plus either a count or a tag to throw. mocha matches the most recent
# expectation first, and a call no expectation admits is an "unexpected
# invocation" that fails the test at the call — both kept here.
#
# WHERE THE CHECK RUNS. CRuby's `Net::HTTP` opens its socket with
# `TCPSocket.open(addr, port, …)`, which is the method mocha replaces;
# the ruby family's copy of this file (`project::TCP_SOCKET_OPEN_REOPEN`,
# appended) reopens `TCPSocket.open` to ask this table first. spinel's
# `net/http` package opens with `TCPSocket.new`, so there the reopened
# client (`runtime/spinel/net_http.rb`, `connect_with_timeout`) asks
# before connecting. Either way the question is asked of the address
# the client is about to connect to — `ipaddr:` when one was pinned —
# which is the fact the tests are about.
#
# Parallel constant Arrays, the shape `resolv.rb` and `http_stub.rb`
# use. The throw tag is a STRING column, thrown as `tag.to_sym`: a
# Symbol read back out of an Array is a poly value, and a `throw` of one
# never meets its `catch` today (matz/spinel#4523).
#
# EXPECT_PREDS is unseeded, as `Resolv::STUB_WHERE` is: the element type
# comes from the lambda the lowered test pushes.
module TcpSocketStub
  EXPECT_PREDS = []
  # Per expectation, at index i + 1 (slot 0 is the seed): the count a
  # counting expectation files (-1 for a throwing one), the tag to
  # throw ("" for a counting one), and the calls seen.
  EXPECT_COUNTS = [ 0 ]
  EXPECT_THROWS = [ "" ]
  EXPECT_CALLS = [ 0 ]

  # `expects(:open).with { … }.never` / `.once` / `.times(n)`.
  def self.expect_open_where(count, pred)
    EXPECT_PREDS << pred
    EXPECT_COUNTS << count
    EXPECT_THROWS << ""
    EXPECT_CALLS << 0
    nil
  end

  # `expects(:open).with { … }.throws(:tag)`.
  def self.expect_open_throws_where(tag, pred)
    EXPECT_PREDS << pred
    EXPECT_COUNTS << -1
    EXPECT_THROWS << tag
    EXPECT_CALLS << 0
    nil
  end

  # The seam. Nothing filed: the client connects. Otherwise the newest
  # expectation whose predicate admits `(host, port)` answers — a throw,
  # or a call counted against its limit — and a call none admits is
  # mocha's unexpected invocation, raised here so the test that made it
  # fails at the call.
  def self.check(host, port)
    return nil if EXPECT_PREDS.length == 0
    i = EXPECT_PREDS.length - 1
    while i >= 0
      if EXPECT_PREDS[i].call(host, port)
        slot = i + 1
        throw EXPECT_THROWS[slot].to_sym if EXPECT_THROWS[slot] != ""
        EXPECT_CALLS[slot] += 1
        if EXPECT_CALLS[slot] > EXPECT_COUNTS[slot]
          raise "unexpected invocation: TCPSocket.open(#{host.inspect}, #{port}) — expected #{EXPECT_COUNTS[slot]} time(s)"
        end
        return nil
      end
      i -= 1
    end
    raise "unexpected invocation: TCPSocket.open(#{host.inspect}, #{port}) — no expectation admits it"
  end

  # Teardown: a counting expectation invoked fewer times than it asked
  # for. (More already raised at the call.)
  def self.verify_open_expectations
    i = 0
    while i < EXPECT_PREDS.length
      slot = i + 1
      if EXPECT_COUNTS[slot] >= 0 && EXPECT_CALLS[slot] < EXPECT_COUNTS[slot]
        raise "not all expectations were satisfied: TCPSocket.open expected #{EXPECT_COUNTS[slot]} time(s), invoked #{EXPECT_CALLS[slot]}"
      end
      i += 1
    end
    nil
  end

  # Setup, between tests: a stub cannot outlive the test that wrote it.
  def self.clear_open_expectations
    EXPECT_PREDS.clear
    EXPECT_COUNTS.clear
    EXPECT_THROWS.clear
    EXPECT_CALLS.clear
    EXPECT_COUNTS << 0
    EXPECT_THROWS << ""
    EXPECT_CALLS << 0
    nil
  end
end
