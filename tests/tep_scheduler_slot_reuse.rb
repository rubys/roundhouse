# The fiber scheduler's slot arrays stay bounded under connection churn.
#
# The real Tep::Scheduler under plain CRuby, over a Sock whose poll set is
# always empty (no fiber here parks on an fd) and a Tep::APP reduced to
# the scheduler's parallel arrays. The scenario is the Scheduled server's:
# the accept fiber spawns one fiber per connection, so the next connection
# is spawned before the previous one's dead slot has reached the tail,
# where tick's reclaim could see it.
class Module
  def ffi_func(*); end
end

require_relative "../runtime/spinel/tep/tep_core"
require_relative "../runtime/spinel/tep/net"
require_relative "../runtime/spinel/tep/scheduler"

module Sock
  def self.sp_net_poll_reset = 0
  def self.sp_net_poll_add(_fd, _mode) = 0
  def self.sp_net_poll_run(_ms) = 0
  def self.sp_net_poll_ready(_slot) = 0
end

class SchedApp
  attr_accessor :sched_fibers, :sched_wake_at, :sched_current
  attr_accessor :sched_io_fd, :sched_io_mode, :sched_io_ready

  def initialize
    @sched_fibers = []
    @sched_wake_at = []
    @sched_io_fd = []
    @sched_io_mode = []
    @sched_io_ready = []
    @sched_current = -1
  end
end

Tep.const_set(:APP, SchedApp.new)

CHECKS = []

def check(name, ok, detail = nil)
  CHECKS << ok
  puts "#{ok ? "ok" : "FAIL"} #{name}#{ok || detail.nil? ? "" : " — #{detail}"}"
end

# A connection that stays open across the whole run (a client holding a
# keep-alive socket): parks once and never wakes. Spawned first, so its
# index is the one every reuse must leave alone.
LONG_LIVED = Fiber.new { Tep::Scheduler.pause(3600) }
Tep::Scheduler.spawn_fiber(LONG_LIVED)
Tep::Scheduler.tick(0)

# Connections that serve one request, wait for a second that never comes,
# and close: alive across one park, dead on the next resume. Each is
# spawned right after the previous one died, before any tick could
# reclaim it -- the accept fiber's timing.
seen = []
CHURN = 200
CHURN.times do
  Tep::Scheduler.spawn_fiber(Fiber.new do
    seen << Tep::APP.sched_current
    Tep::Scheduler.pause(0)
    0
  end)
  Tep::Scheduler.tick(0) # the new fiber runs its request and parks
  Tep::Scheduler.tick(0) # ...and closes
end

check(
  "#{CHURN} churned connections leave the slot arrays bounded",
  Tep::APP.sched_fibers.length <= 2,
  "#{Tep::APP.sched_fibers.length} slots"
)
check(
  "every churned connection ran in the slot the first one vacated",
  seen.uniq == [1],
  "slots used: #{seen.uniq.length}"
)
check(
  "the long-lived connection kept its slot",
  Tep::APP.sched_fibers[0].f.equal?(LONG_LIVED) && LONG_LIVED.alive?
)
check(
  "the parallel arrays stay the same length",
  [Tep::APP.sched_wake_at, Tep::APP.sched_io_fd, Tep::APP.sched_io_mode, Tep::APP.sched_io_ready]
    .all? { |a| a.length == Tep::APP.sched_fibers.length }
)

# With nothing spawned behind it, the last dead slot is at the tail and the
# next tick's reclaim takes it: the arrays shrink back to the live set.
Tep::Scheduler.tick(0)
check(
  "an idle tick reclaims the trailing dead slot",
  Tep::APP.sched_fibers.length == 1 && Tep::Scheduler.alive_count == 1,
  "#{Tep::APP.sched_fibers.length} slots, #{Tep::Scheduler.alive_count} alive"
)

puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
puts "done"
