# Tep::WebSocket::Connection -- per-connection recv loop.
#
# Designed to run inside a Tep::Scheduler-managed fiber spawned by
# the upgrade route after the 101 response is written. The fiber:
#   1. Parks on Tep::Scheduler.io_wait(fd, READ, timeout) for bytes.
#   2. Reads via Sock.sp_net_recv_some(:binstr) into a binary String,
#      appended to a per-connection accumulator (binary-safe `+`).
#   3. Walks the accumulator with Frame.parse_from_buf, dispatching
#      events to the Driver's handlers and carrying any partial trailing
#      frame forward via byteslice.
#   4. On close (sent OR received), exits cleanly + closes the fd.
#
# The accumulator is a per-fiber Ruby String (binary-safe via :binstr +
# pack), so it needs no shared static buffer. A future Phase 2.1 (or
# whenever multi-fiber WS
# concurrency-per-worker becomes a goal) replaces this with
# per-fiber buffers via Fiber.storage (matz/spinel#578).
module Tep
  module WebSocket
    class Connection
      attr_accessor :driver, :fd, :idle_timeout_seconds

      def initialize(driver)
        @driver = driver
        @fd     = driver.fd
        @idle_timeout_seconds = 300
      end

      def set_idle_timeout(seconds)
        @idle_timeout_seconds = seconds
      end

      # Drive the recv loop. Returns 0 on clean close, -1 on error.
      # Idempotent across multiple frames per recv: a single
      # sphttp_recv_into_frame fill may contain several complete
      # frames; Connection consumes them all before parking again.
      #
      # The caller (Tep::Server::Scheduled.write_response) owns the
      # fd lifecycle -- run() never calls sphttp_close. On clean
      # close OR error the server's handle_connection closes the fd
      # via its usual exit path.
      def run
        # Synthetic open event before the first recv -- handlers
        # often want to send a welcome message.
        Connection.dispatch_open(@driver)

        # Inbound byte accumulator. sp_net_recv_some(:binstr) returns a
        # binary-safe String; append it (binary-safe `+`) and parse as many
        # whole frames as the buffer holds, carrying any partial trailing
        # frame forward via byteslice. Replaces the old static recv buffer
        # + per-byte C accessor (sphttp.c is retired — matz/spinel#1466).
        inbuf = ""
        while true
          ready = Tep::Scheduler.io_wait(@fd, Tep::Scheduler::READ, @idle_timeout_seconds)
          if ready == 0
            # Timeout: close 1001 going-away.
            @driver.close(Tep::WebSocket::CLOSE_GOING_AWAY, "idle timeout")
            return 0
          end

          chunk = Sock.sp_net_recv_some(@fd, 65536)
          if chunk.length == 0
            # EOF / peer gone: dispatch close without sending one back.
            Connection.dispatch_close(@driver, Tep::WebSocket::CLOSE_GOING_AWAY, "")
            return 0
          end
          inbuf = inbuf + chunk

          # Parse + dispatch every complete frame the buffer now holds.
          while true
            r = Tep::WebSocket::Frame.parse_from_buf(inbuf, 0, inbuf.length)
            if r.outcome == "need"
              break
            end
            if r.outcome == "close"
              @driver.close(r.close_code, "protocol error")
              return 0
            end
            Connection.dispatch_frame(@driver, r.frame)
            if r.consumed >= inbuf.length
              inbuf = ""
              break
            end
            inbuf = inbuf.byteslice(r.consumed, inbuf.length - r.consumed)
          end
        end
        0
      end

      # Route a parsed frame to the right handler.
      def self.dispatch_frame(driver, frame)
        op = frame.opcode
        if op == Tep::WebSocket::OPCODE_TEXT
          Connection.dispatch_message(driver, frame.payload, true)
        elsif op == Tep::WebSocket::OPCODE_BINARY
          Connection.dispatch_message(driver, frame.payload, false)
        elsif op == Tep::WebSocket::OPCODE_PING
          # Auto-pong with the ping's payload (§5.5.3).
          driver.pong(frame.payload)
          Connection.dispatch_ping(driver, frame.payload)
        elsif op == Tep::WebSocket::OPCODE_PONG
          Connection.dispatch_pong(driver, frame.payload)
        elsif op == Tep::WebSocket::OPCODE_CLOSE
          code = 0
          reason = ""
          if frame.payload.length >= 2
            code = (frame.payload[0].ord << 8) | frame.payload[1].ord
            if frame.payload.length > 2
              reason = frame.payload[2, frame.payload.length - 2]
            end
          end
          # Echo the close back (§5.5.1) then dispatch.
          driver.close(code == 0 ? Tep::WebSocket::CLOSE_NORMAL : code, reason)
          Connection.dispatch_close(driver, code, reason)
        end
        0
      end

      def self.dispatch_open(driver)
        evt = Tep::WebSocket::Event.new
        driver.h_open.handle_event(evt)
        0
      end

      def self.dispatch_message(driver, data, text)
        evt = Tep::WebSocket::Event.new
        evt.data = data
        driver.h_message.handle_event(evt)
        0
      end

      def self.dispatch_ping(driver, data)
        evt = Tep::WebSocket::Event.new
        evt.data = data
        driver.h_ping.handle_event(evt)
        0
      end

      def self.dispatch_pong(driver, data)
        evt = Tep::WebSocket::Event.new
        evt.data = data
        driver.h_pong.handle_event(evt)
        0
      end

      def self.dispatch_close(driver, code, reason)
        evt = Tep::WebSocket::Event.new
        evt.code = code
        evt.reason = reason
        driver.h_close.handle_event(evt)
        # Auto-cleanup: any Broadcast subscription or Presence row
        # keyed on this connection's fd gets dropped. Both calls
        # are no-op-safe when nothing was tracked (zero matches).
        # Apps that still call unsubscribe_fd / untrack_by_fd
        # explicitly stay correct -- the second call finds 0 matches.
        Tep::Broadcast.unsubscribe_fd(driver.fd)
        Tep::Presence.untrack_by_fd(driver.fd)
        0
      end
    end
  end
end
