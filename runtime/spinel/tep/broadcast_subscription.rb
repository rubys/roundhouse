# Tep::BroadcastSubscription -- one entry in the Tep::Broadcast
# subscriber registry. Pairs a topic name with an output: the
# connection's Tep::WebSocket::Driver for framed delivery, or a bare
# fd for raw bytes (server-sent events, log fan-out).
#
# WHY THE DRIVER AND NOT JUST ITS FD. A publish runs on whichever thread
# committed the record, and it writes after copying the matching entries
# out of the registry. Between that copy and the write the subscribing
# connection can close and the kernel can hand its fd number to the next
# accept -- so a write addressed to a NUMBER can land in a stranger's
# socket. The driver's `write_frame` holds the connection's write lock
# and refuses once the fd's owner has retired it, and that is the only
# way a framed payload reaches the wire. `fd` stays alongside for the
# raw-bytes mode and for `unsubscribe_fd`, which the closing connection
# calls while the number is still uniquely its own.
#
# Each subscription lives in a single worker's registry. Cross-
# worker pub-sub goes through PG LISTEN/NOTIFY (see
# Tep::Broadcast.enable_pg_backend) which fans publishes out
# without moving subscription state; subscribers always register
# fd-local. See docs/BATTERIES-DESIGN.md for the broader Broadcast
# battery design.
module Tep
  class BroadcastSubscription
    attr_reader :topic   # String
    attr_reader :fd      # Integer file descriptor
    # Delivery mode controls how Tep::Broadcast.publish writes
    # `payload` to `fd`:
    #
    #   0 = raw bytes (Sock.sphttp_write_str). The default; suits
    #       SSE / log fan-out / anything that doesn't need framing.
    #   1 = WebSocket TEXT frame (Tep::WebSocket::OPCODE_TEXT).
    #   2 = WebSocket BINARY frame (Tep::WebSocket::OPCODE_BINARY).
    #
    # Modes 1 and 2 route through Tep::WebSocket::Driver.send_frame,
    # so payloads land as proper WS frames the peer will accept.
    # Apps register mode-1 subscriptions via subscribe_ws.
    attr_reader :mode
    attr_reader :ws      # Tep::WebSocket::Driver; the writer for modes 1 and 2

    def initialize(topic, fd, mode, ws)
      @topic = topic
      @fd    = fd
      @mode  = mode
      @ws    = ws
    end
  end
end
