# Roundhouse Crystal cable runtime — scaffolding only.
#
# Parity with runtime/rust/cable.rs and runtime/python/cable.py is
# scoped to a later pass. For now `handle` acknowledges the route
# exists so the server's `/cable` registration compiles, but
# doesn't upgrade the WebSocket. Client-side `@rails/actioncable`
# attempts to connect and fails quietly — navigation + form-submit
# flows (what the compare tool exercises) don't depend on cable.

require "http/server"

module Roundhouse
  module Cable
    # Returns 426 Upgrade Required so a curl probe sees a definitive
    # status; a real browser interpreting the response just moves
    # on to degraded mode.
    def self.handle(context : HTTP::Server::Context) : Nil
      context.response.status_code = 426
      context.response.content_type = "text/plain"
      context.response.print "WebSocket upgrade not wired yet"
    end

    # Stub broadcaster entry — models with `broadcasts_to` that the
    # emitter lowers later will call through here.
    def self.broadcast(channel : String, body : String) : Nil
      _ = channel
      _ = body
    end
  end
end
