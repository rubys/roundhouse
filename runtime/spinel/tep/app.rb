# Tep::App -- per-process singleton holding the fan-out registries the
# connection threads share.
#
# This is a heavily trimmed vendoring of upstream tep's lib/tep/app.rb.
# Upstream's App also carries the router, filter slots, auth/oauth,
# presence, PG pub/sub, asset bodies, and session handling, and its
# `dispatch` walks that whole pipeline. Roundhouse dispatches through
# its own `Main.dispatch` (see scaffold/main.rb), so this copy keeps
# only what Tep::Broadcast and Cable read off `Tep::APP`, plus a
# `dispatch` that leases a connection and delegates to the roundhouse
# app.
#
# Connections are green threads (Tep::Server::Threaded), so the two
# registries here are shared between threads and each has a lock; every
# reader and writer takes it (tep/broadcast.rb, cable.rb).
module Tep
  class App
    # Broadcast subscriber registry (Tep::Broadcast). Each entry pairs
    # a topic with an output fd + delivery mode.
    attr_accessor :broadcast_subs, :broadcast_lock

    # Action Cable stream -> identifier-JSON map (Cable). Lives on the
    # singleton (not a Cable module constant) because spinel reliably
    # types a Tep.str_hash ivar as StrStrHash but mistypes a
    # module-level constant initialised the same way as int. Same
    # rationale that puts broadcast_subs here rather than on a module.
    attr_accessor :cable_identifiers, :cable_lock

    # Live cable connections, one Driver per open WebSocket, and the
    # flag that says the process's ONE heartbeat thread is already
    # running. Cable registers on open and unregisters on close, the
    # same lifecycle as a broadcast subscription, and the heartbeat
    # walks the list every Cable::PING_INTERVAL. Here rather than in
    # Cable for the reason `cable_identifiers` is: a module-level
    # constant seeded the same way types as int.
    attr_accessor :cable_conns, :cable_conns_lock, :cable_heartbeat

    # Cooperative-scheduler state (Tep::Scheduler; only the
    # TEP_SERVER=fiber measurement lane runs it). One entry per fiber
    # across these parallel arrays:
    #   sched_fibers   PtrArray<FiberSlot>  the Fiber
    #   sched_wake_at  IntArray             unix-seconds; -1 = ready now
    #   sched_io_fd    IntArray             fd parked on; -1 = none
    #   sched_io_mode  IntArray             requested mode bits (1=R,2=W)
    #   sched_io_ready IntArray             observed-ready bits (0=not yet)
    attr_accessor :sched_fibers, :sched_wake_at, :sched_current
    attr_accessor :sched_io_fd, :sched_io_mode, :sched_io_ready

    # The app's own name, for the startup banner and the log prefix
    # (Tep.display_name). Set by scaffold/main.rb; empty until then.
    attr_accessor :name

    def initialize
      @name = ""
      # Type-seed the Broadcast subscriber registry: a first push pins
      # the element type, then it is dropped.
      @broadcast_subs = [Tep::BroadcastSubscription.new(
        "_", -1, 0, Tep::WebSocket::Driver.new(-1))]
      @broadcast_subs.pop
      @broadcast_lock = Mutex.new

      # Cable stream -> identifier-JSON map.
      @cable_identifiers = Tep.str_hash
      @cable_lock = Mutex.new

      # Live cable connections. Seed-and-drop pins the element type, the
      # same way broadcast_subs above does.
      @cable_conns = [Tep::WebSocket::Driver.new(-1)]
      @cable_conns.pop
      @cable_conns_lock = Mutex.new
      @cable_heartbeat = 0

      # Scheduler arrays (fiber lane). FiberSlot array -- seed with a
      # noop-bodied slot to pin the element type, then drop it.
      @sched_fibers   = [Tep::FiberSlot.new(Fiber.new { Tep.seed_fiber_noop })]
      @sched_fibers.pop
      @sched_wake_at  = [0]
      @sched_wake_at.pop
      @sched_current  = -1               # currently-running fiber idx
      @sched_io_fd    = [0]
      @sched_io_fd.pop
      @sched_io_mode  = [0]
      @sched_io_mode.pop
      @sched_io_ready = [0]
      @sched_io_ready.pop
    end

    # Tep::Server::Threaded calls Tep::APP.dispatch(req, res) per
    # request (its cmeth handler bodies can't carry instance state, so
    # the app handle lives on the singleton). Roundhouse's request
    # pipeline is Main.dispatch.
    #
    # THE LEASE LIVES HERE, because this is the path every request takes.
    # `Db.with_connection` leases one pooled connection to the request,
    # switches on the per-request query cache, and trims the statement
    # cache at the boundary. It used to sit on `MainApp#dispatch` in the
    # scaffold, which the server is handed but never calls -- so no
    # request leased anything: every fiber shared the pool's first
    # handle, the statement cache was never trimmed in the binary, and a
    # query cache wired to the lease could never turn on. The `/cable`
    # upgrade is dispatched under this lease too; the socket's recv loop
    # runs after the response and outside it, as before.
    def dispatch(req, res)
      Db.with_connection { Main.dispatch(req, res) }
    end
  end

  APP = App.new
end
