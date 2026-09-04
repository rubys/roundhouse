#!/usr/bin/env ruby
# The SPINEL lane's cable HEARTBEAT and its connection registry,
# exercised.
#
# Loads `runtime/spinel/cable.rb` directly — no emit, no server, no
# socket: `ruby tests/spinel_cable_heartbeat.rb .` reproduces it by
# hand.
#
# WHY THIS FILE EXISTS. The heartbeat used to be a green thread per
# connection, which cost the scheduler one monitor turn per beat once
# the connections' phases spread out (matz/spinel#4317, and the ledger
# entry the runtime doc carries). One thread now beats for every open
# connection, which means the set of open connections is a THING — a
# registry — and a registry has exactly one way to go wrong that a
# per-connection thread could not: it can keep beating for connections
# that are gone. A closed connection's driver refuses the write, so a
# leak is SILENT at the socket and shows up only as work: nothing a
# client can observe, which is why it needs a test that can see the
# registry rather than a probe that counts received pings.
#
# THE TRANSPORT IS FAKED HERE, deliberately unlike its two siblings.
# `spinel_cable_identity.rb` and `spinel_cable_channel.rb` stub
# `Tep::WebSocket::Handler` EMPTY so that a probe which reaches the
# transport dies naming the constant — for them the transport is out of
# scope. Here the transport IS the subject, so the driver is a recording
# fake: it answers `fd` and counts what was written to it.
root = ARGV[0] or abort "usage: spinel_cable_heartbeat.rb <repo-root>"
root = File.expand_path(root)

require "json"

module Rails
  Application = Struct.new(:secret_key_base)
  def self.application = @app ||= Application.new("a" * 64)
end

load "#{root}/runtime/spinel/message_digest_cruby.rb"
load "#{root}/runtime/ruby/action_controller/message_verifier.rb"
load "#{root}/runtime/ruby/action_controller/cookies.rb"

module Tep
  module WebSocket
    class Handler; end

    # The recording fake. `fd` is what the registry matches on —
    # `Cable.unregister` drops by fd number, the way
    # `Tep::Broadcast.unsubscribe_fd` does, because the number is
    # uniquely this connection's until it closes. `text` counts, and
    # answers -1 once retired, which is what the real
    # `Driver#write_frame` does behind its lock.
    class Driver
      attr_reader :fd, :writes
      def initialize(fd)
        @fd = fd
        @writes = []
        @retired = false
      end

      def retire
        @retired = true
        0
      end

      def text(s)
        return -1 if @retired
        @writes << s
        s.bytesize
      end
    end
  end

  # Only the slots `cable.rb` reads. The real one is `Tep::App`;
  # `Tep::APP` is the per-process singleton it reaches for.
  class FakeApp
    attr_accessor :cable_conns, :cable_conns_lock, :cable_heartbeat
    attr_accessor :cable_identifiers, :cable_lock
    def initialize
      @cable_conns = []
      @cable_conns_lock = Mutex.new
      @cable_heartbeat = 0
      @cable_identifiers = {}
      @cable_lock = Mutex.new
    end
  end
  APP = FakeApp.new
end

load "#{root}/runtime/spinel/action_cable.rb"   # requires cable.rb

puts "vendor ruby #{RUBY_VERSION}"

FAILURES = []

def check(label, got, want)
  if got == want
    puts "  ok   #{label}"
  else
    FAILURES << label
    puts "  FAIL #{label}: got #{got.inspect}, want #{want.inspect}"
  end
end

def conns = Tep::APP.cable_conns

# The heartbeat thread is not wanted in a unit test — it would sleep
# three seconds and beat on its own clock. `Cable.register` starts it
# once and only once, and it decides that from the flag, so the probes
# below assert the flag transition and pre-arm it rather than letting a
# thread loose in the process.
spawned = 0
Cable.define_singleton_method(:spawn_heartbeat) { spawned += 1; 0 }

# --- registration ----------------------------------------------------
a = Tep::WebSocket::Driver.new(11)
b = Tep::WebSocket::Driver.new(12)
c = Tep::WebSocket::Driver.new(13)

check("the registry starts empty", conns.length, 0)
check("no heartbeat before the first connection", Tep::APP.cable_heartbeat, 0)

Cable.register(a)
check("the first open registers", conns.length, 1)
check("the first open starts the heartbeat", spawned, 1)
check("and marks it started", Tep::APP.cable_heartbeat, 1)

Cable.register(b)
Cable.register(c)
check("later opens register", conns.length, 3)
check("ONE heartbeat for the process, not one per connection", spawned, 1)

# --- the beat --------------------------------------------------------
n = Cable.beat
check("a beat writes to every open connection", n, 3)
check("  and each got exactly one frame", conns.map { |d| d.writes.length }, [1, 1, 1])
frame = a.writes.first
check("the frame is an Action Cable ping", JSON.parse(frame)["type"], "ping")
check("carrying a unix timestamp", JSON.parse(frame)["message"].is_a?(Integer), true)
check("one timestamp for the whole beat, read once",
      conns.map { |d| JSON.parse(d.writes.first)["message"] }.uniq.length, 1)

# --- close ------------------------------------------------------------
# The real close path retires the driver and then closes the fd; the
# registry has to lose the entry, or the beat keeps paying for it
# forever. This is the failure the per-connection thread could not have:
# that thread exited on the refused write.
b.retire
Cable.unregister(b)
check("a closed connection leaves the registry", conns.length, 2)
check("  and it is the right one", conns.map(&:fd), [11, 13])

Cable.beat
check("the beat no longer writes to it", b.writes.length, 1)
check("and still writes to the rest", conns.map { |d| d.writes.length }, [2, 2])

# THE CONTRACT, stated as a probe. The registry matches by fd NUMBER,
# which the kernel hands to the next `accept` the moment the old owner
# closes — so unregister runs on the close path BEFORE the fd is closed,
# while the number still names only this connection. Registering the
# number's next owner and then unregistering the OLD driver takes the
# new one out, which is not a bug to fix here but the reason the
# ordering is not negotiable; `Tep::Broadcast.unsubscribe_fd` carries
# the identical contract and the identical comment.
d = Tep::WebSocket::Driver.new(12)
Cable.register(d)
check("the number's next owner registers", conns.map(&:fd), [11, 13, 12])
check("unregistering by a STALE number would take the new owner: run it before the close",
      Cable.unregister(b), 1)
check("  which is what it did", conns.map(&:fd), [11, 13])

# --- empty ------------------------------------------------------------
Cable.unregister(a)
Cable.unregister(c)
check("the last close empties the registry", conns.length, 0)
check("a beat over nobody is not an error", Cable.beat, 0)
check("and the heartbeat is still marked running", Tep::APP.cable_heartbeat, 1)
check("  without a second thread", spawned, 1)

if FAILURES.empty?
  puts "spinel cable heartbeat: all probes pass"
else
  puts "spinel cable heartbeat: #{FAILURES.length} FAILED"
  exit 1
end
