# Tep::BroadcastSubscription -- one entry in the Tep::Broadcast
# subscriber registry. Pairs a topic name with an output fd. When a
# publish matches the topic, the fd gets the payload bytes via
# Sock.sphttp_write_str.
#
# fd is just an integer file descriptor: typically a WebSocket
# connection's accepted socket fd, but the registry doesn't care
# about the protocol on top -- it'll write to any open fd. Apps
# integrating with WS (via Tep::WebSocket) subscribe their
# connection fds; non-WS use cases (server-sent events, log
# fan-out, etc.) work the same way.
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

    def initialize(topic, fd, mode)
      @topic = topic
      @fd    = fd
      @mode  = mode
    end
  end
end
