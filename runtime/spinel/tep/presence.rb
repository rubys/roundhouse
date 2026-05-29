# Tep::Presence -- stub.
#
# Upstream tep ships a full Presence battery (who's-online tracking
# with optional PG mirroring). Roundhouse doesn't use it, but
# Tep::WebSocket::Connection.dispatch_close calls
# Tep::Presence.untrack_by_fd on every socket close to drop any
# presence rows keyed on that fd. This stub satisfies that call as a
# no-op so the WebSocket teardown path resolves without vendoring the
# whole battery (which pulls presence_entry + PG).
module Tep
  module Presence
    def self.untrack_by_fd(fd)
      0
    end
  end
end
