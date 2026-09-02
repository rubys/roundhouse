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

    def initialize
      # Type-seed the Broadcast subscriber registry: a first push pins
      # the element type, then it is dropped.
      @broadcast_subs = [Tep::BroadcastSubscription.new("_", -1, 0)]
      @broadcast_subs.pop
      @broadcast_lock = Mutex.new

      # Cable stream -> identifier-JSON map.
      @cable_identifiers = Tep.str_hash
      @cable_lock = Mutex.new
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
