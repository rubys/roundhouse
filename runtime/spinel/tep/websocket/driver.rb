# Tep::WebSocket::Driver -- Faye-shape state machine + event dispatch.
#
# Constructed AFTER the handshake completes, before the recv loop
# starts. Holds per-connection state + outbound write methods +
# the event-callback registry that the handler (or the Phase 3 DSL)
# populates.
#
# Faye-shape API (matches faye/websocket-driver-ruby's surface for
# the parts tep ships -- single-frame text/binary, ping/pong, close):
#
#     drv = Tep::WebSocket::Driver.new(fd)
#     drv.on_message    do |evt| ... end    # block-based on:open/on:message etc
#     drv.on_close      do |evt| ... end    #   are syntactic sugar; tep ships
#     drv.text("hi")                        #   the explicit setters instead
#     drv.binary(bytes)
#     drv.ping("")
#     drv.close(1000, "bye")
#
# In Phase 2, callbacks are set via explicit setters (`set_on_message`)
# rather than `on(:message) { block }` since spinel's block-with-
# closure-on-locals support is still uneven outside Fiber.new bodies.
# Phase 3's DSL hides this behind `ws.on(:message) { ... }` once we
# decide on the lowering shape.
module Tep
  module WebSocket
    class Driver
      attr_accessor :fd, :max_frame_size, :subprotocol
      # Callback slots. Each holds a subclass of Tep::WebSocket::Handler
      # (or the base) that gets `handle_event(event)` called when the
      # corresponding wire event arrives. Defaults to a no-op base
      # so the slot is type-safe pre-set.
      attr_accessor :h_open, :h_message, :h_close, :h_ping, :h_pong, :h_error

      def initialize(fd)
        @fd             = fd
        @max_frame_size = Tep::WebSocket::DEFAULT_MAX_FRAME
        @subprotocol    = ""
        @h_open    = Tep::WebSocket::Handler.new
        @h_message = Tep::WebSocket::Handler.new
        @h_close   = Tep::WebSocket::Handler.new
        @h_ping    = Tep::WebSocket::Handler.new
        @h_pong    = Tep::WebSocket::Handler.new
        @h_error   = Tep::WebSocket::Handler.new
      end

      def set_max_frame_size(n)
        @max_frame_size = n
      end

      # Reassign the underlying fd. Used by the server-side upgrade
      # path: the user handler builds the Driver with a placeholder
      # fd (since the client fd isn't visible at handler-dispatch
      # time), and the write_response branch sets the real fd here
      # right before constructing the Connection.
      def set_fd(new_fd)
        @fd = new_fd
      end

      def set_subprotocol(name)
        @subprotocol = name
      end

      def set_on_open(h);    @h_open = h;    end
      def set_on_message(h); @h_message = h; end
      def set_on_close(h);   @h_close = h;   end
      def set_on_ping(h);    @h_ping = h;    end
      def set_on_pong(h);    @h_pong = h;    end
      def set_on_error(h);   @h_error = h;   end

      # Send a text frame.
      def text(s)
        Driver.send_frame(@fd, Tep::WebSocket::OPCODE_TEXT, s)
      end

      # (Upstream tep also defines a Streamer-shape `write(s)` alias for
      # `text`, used by Tep::Llm.chat_stream. Roundhouse doesn't vendor
      # Llm and never calls it; dropped here because an uncalled
      # String-param method's param defaults to int under spinel and
      # then fails the C-compile when its body passes it as a string.)

      # Send a binary frame.
      def binary(bytes)
        Driver.send_frame(@fd, Tep::WebSocket::OPCODE_BINARY, bytes)
      end

      # Send a ping with optional payload (<=125 bytes).
      def ping(payload)
        Driver.send_frame(@fd, Tep::WebSocket::OPCODE_PING, payload)
      end

      # Send a pong with the matching ping's payload (per §5.5.3).
      def pong(payload)
        Driver.send_frame(@fd, Tep::WebSocket::OPCODE_PONG, payload)
      end

      # Send a close frame with code + reason. Reason capped at
      # 123 bytes so the 2-byte code + reason fits in a control
      # frame's 125-byte payload limit.
      def close(code, reason)
        body = Driver.encode_close_payload(code, reason)
        Driver.send_frame(@fd, Tep::WebSocket::OPCODE_CLOSE, body)
      end

      # Build the frame bytes (unmasked, server-side) and write via
      # sphttp_write_bytes (binary-safe, explicit length).
      def self.send_frame(fd, opcode, payload)
        frame = Tep::WebSocket::Frame.new(true, opcode, payload)
        bytes = frame.encode_unmasked
        Sock.sphttp_write_bytes(fd, bytes, bytes.length)
      end

      # Close payload: 2-byte big-endian code + UTF-8 reason. Per
      # §5.5.1 the payload may be omitted (close with no body); if
      # `code == 0` we emit an empty payload.
      def self.encode_close_payload(code, reason)
        if code == 0
          return ""
        end
        out = Tep::WebSocket::Frame.byte_to_chr((code >> 8) & 0xff) +
              Tep::WebSocket::Frame.byte_to_chr(code & 0xff)
        if reason.length > 123
          out + reason[0, 123]
        else
          out + reason
        end
      end
    end

    # Event passed to handler callbacks. Holds `data` (the payload
    # as String for text/binary, raw bytes for ping/pong, or the
    # close code+reason for close) and a numeric `code` for close.
    class Event
      attr_accessor :data, :code, :reason

      def initialize
        @data   = ""
        @code   = 0
        @reason = ""
      end
    end

    # Base class for event handlers. Subclass + override
    # `handle_event(event)`. The Driver stores one Handler instance
    # per event type and dispatches via `@h_message.handle_event(evt)`.
    # The explicit-Handler shape (vs faye's block-based `driver.on(:msg)
    # { ... }`) is chosen because it stays compatible with future
    # Fiber.storage per-connection state plumbing without re-typing
    # the callback boundary.
    #
    # `req` is set at WS upgrade time by the route handler the
    # translator emits, giving on_X handler bodies access to the
    # request that initiated the connection (req.identity,
    # req.session, headers, ...). It stays the same across every
    # event on the connection -- there's no per-frame "request".
    class Handler
      attr_accessor :req

      def initialize
        @req = Tep::Request.new
      end

      def handle_event(event)
        0
      end
    end
  end
end
