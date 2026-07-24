# Tep::App -- per-worker singleton holding the cooperative scheduler's
# parallel fiber arrays + the Broadcast subscriber registry.
#
# This is a heavily trimmed vendoring of upstream tep's lib/tep/app.rb.
# Upstream's App also carries the router, filter slots, auth/oauth,
# presence, PG pub/sub, asset bodies, and session handling, and its
# `dispatch` walks that whole pipeline. Roundhouse dispatches through
# its own `Main.dispatch` (see scaffold/main.rb), so this copy keeps
# only the two pieces Tep::Scheduler and Tep::Broadcast read off
# `Tep::APP`, plus a `dispatch` that delegates to the roundhouse app.
module Tep
  class App
    # Cooperative-scheduler state (Tep::Scheduler). One entry per
    # fiber across these parallel arrays:
    #   sched_fibers   PtrArray<FiberSlot>  the Fiber
    #   sched_wake_at  IntArray             unix-seconds; -1 = ready now
    #   sched_io_fd    IntArray             fd parked on; -1 = none
    #   sched_io_mode  IntArray             requested mode bits (1=R,2=W)
    #   sched_io_ready IntArray             observed-ready bits (0=not yet)
    attr_accessor :sched_fibers, :sched_wake_at, :sched_current
    attr_accessor :sched_io_fd, :sched_io_mode, :sched_io_ready

    # Broadcast subscriber registry (Tep::Broadcast). Each entry pairs
    # a topic with an output fd + delivery mode.
    attr_accessor :broadcast_subs

    # Action Cable stream -> identifier-JSON map (Cable). Lives on the
    # singleton (not a Cable module constant) because spinel reliably
    # types a Tep.str_hash ivar as StrStrHash but mistypes a
    # module-level constant initialised the same way as int. Same
    # rationale that puts broadcast_subs here rather than on a module.
    attr_accessor :cable_identifiers

    def initialize
      # FiberSlot array — seed with a noop-bodied slot to pin the
      # array element type, then drop it.
      @sched_fibers   = [Tep::FiberSlot.new(Fiber.new { Tep.seed_fiber_noop })]
      @sched_fibers.clear
      @sched_wake_at  = [0]
      @sched_wake_at.clear
      @sched_current  = -1               # currently-running fiber idx
      @sched_io_fd    = [0]
      @sched_io_fd.clear
      @sched_io_mode  = [0]
      @sched_io_mode.clear
      @sched_io_ready = [0]
      @sched_io_ready.clear

      # Type-seed the Broadcast subscriber registry the same way.
      @broadcast_subs = [Tep::BroadcastSubscription.new("_", -1, 0)]
      @broadcast_subs.clear

      # Cable stream -> identifier-JSON map.
      @cable_identifiers = Tep.str_hash
    end

    # Tep::Server::Scheduled calls Tep::APP.dispatch(req, res) per
    # request (its cmeth handler bodies can't carry instance state, so
    # the app handle lives on the singleton). Roundhouse's request
    # pipeline is Main.dispatch; delegate straight to it.
    def dispatch(req, res)
      Main.dispatch(req, res)
    end
  end

  APP = App.new
end
