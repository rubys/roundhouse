# Tep::WebSocket::Connection -- per-connection recv loop.
#
# Designed to run inside a Tep::Scheduler-managed fiber spawned by
# the upgrade route after the 101 response is written. The fiber:
#   1. Parks on Tep::Scheduler.io_wait(fd, READ, timeout) for bytes.
#   2. Reads via Sock.sphttp_recv_into_frame into the binary frame buf.
#   3. Walks the accumulated buffer with Frame.parse_from_buf,
#      dispatching events to the Driver's handlers.
#   4. On close (sent OR received), exits cleanly + closes the fd.
#
# The recv buffer (sphttp_frame_buf, 64 KiB) is the per-fork static
# from Phase 0.5; cross-fiber sharing within one worker process is
# bounded by the worker's cooperative scheduling -- only one fiber
# parses at a time. A future Phase 2.1 (or whenever multi-fiber WS
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

        while true
          ready = Tep::Scheduler.io_wait(@fd, Tep::Scheduler::READ, @idle_timeout_seconds)
          if ready == 0
            # Timeout: close 1001 going-away.
            @driver.close(Tep::WebSocket::CLOSE_GOING_AWAY, "idle timeout")
            return 0
          end

          n = Sock.sphttp_recv_into_frame(@fd)
          if n <= 0
            # EOF or error: dispatch close without sending one back
            # (peer already gone) and exit.
            Connection.dispatch_close(@driver, Tep::WebSocket::CLOSE_GOING_AWAY, "")
            if n == 0
              return 0
            end
            return -1
          end

          # Parse + dispatch as many complete frames as possible
          # from this recv.
          state = Tep::WebSocket::ConnectionState.new
          state.start = 0
          state.avail = n
          while true
            r = Tep::WebSocket::Frame.parse_from_buf(state.start, state.avail)
            if r.outcome == "need"
              break
            end
            if r.outcome == "close"
              @driver.close(r.close_code, "protocol error")
              return 0
            end
            Connection.dispatch_frame(@driver, r.frame)
            state.start = state.start + r.consumed
            if state.start >= state.avail
              break
            end
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

    # Per-recv-loop iteration state. Avoids tuple-returns from
    # parse_from_buf calls (spinel multi-return support is uneven).
    class ConnectionState
      attr_accessor :start, :avail
      def initialize
        @start = 0
        @avail = 0
      end
    end
  end
end
