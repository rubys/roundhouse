# Compile the real scheduler with Spinel and run the churn scenario of
# tests/tep_scheduler_slot_reuse.rb on it: a long-lived fiber first, then
# 200 fibers that each park once and finish, every one spawned before the
# previous one's dead slot could reach the tail. Tep::APP is reduced to
# the scheduler's parallel arrays (app.rb's own seeding), since the full
# App pulls in the broadcast and WebSocket stack.
require_relative "../runtime/spinel/tep/tep_core"
require_relative "../runtime/spinel/tep/net"

module Tep
  class App
    attr_accessor :sched_fibers, :sched_wake_at, :sched_current
    attr_accessor :sched_io_fd, :sched_io_mode, :sched_io_ready

    def initialize
      @sched_fibers   = [Tep::FiberSlot.new(Fiber.new { Tep.seed_fiber_noop })]
      @sched_fibers.pop
      @sched_wake_at  = [0]
      @sched_wake_at.pop
      @sched_current  = -1
      @sched_io_fd    = [0]
      @sched_io_fd.pop
      @sched_io_mode  = [0]
      @sched_io_mode.pop
      @sched_io_ready = [0]
      @sched_io_ready.pop
    end
  end

  APP = App.new
end

require_relative "../runtime/spinel/tep/scheduler"

def churn_body
  Tep::Scheduler.pause(0)
  0
end

def long_body
  Tep::Scheduler.pause(3600)
  0
end

Tep::Scheduler.spawn_fiber(Fiber.new { long_body })
Tep::Scheduler.tick(0)
i = 0
while i < 200
  Tep::Scheduler.spawn_fiber(Fiber.new { churn_body })
  Tep::Scheduler.tick(0)
  Tep::Scheduler.tick(0)
  i += 1
end
puts "slots after churn: " + Tep::APP.sched_fibers.length.to_s
Tep::Scheduler.tick(0)
puts "slots after idle tick: " + Tep::APP.sched_fibers.length.to_s
puts "alive: " + Tep::Scheduler.alive_count.to_s
