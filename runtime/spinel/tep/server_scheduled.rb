# Tep::Server::Scheduled -- Falcon-shape fiber-per-connection HTTP
# server, built on Tep::Scheduler + sphttp non-blocking accept/recv.
#
# Why this exists
# ---------------
# The default Tep::Server (in server.rb) is prefork + blocking per
# worker -- N workers <=> N concurrent connections. WebSockets and
# slow keep-alive clients tie up a worker for the full connection
# duration, so the prefork pool's effective concurrency degrades
# to N regardless of actual CPU work. The scheduled variant accepts
# in a fiber, spawns one fiber per accepted connection, and parks
# all I/O on Tep::Scheduler.io_wait -- N workers serve M >> N
# concurrent connections, bounded only by per-fiber memory.
#
# Fiber bodies use ordinary closure capture for sfd / client now
# (matz/spinel#564 + #1007 both closed; the heap-cell-reset fix
# in spinel commit 48594d6 lets multi-method capture chains lower
# correctly). cmeths still preferred for accept_loop /
# handle_connection so the bodies read cleanly without per-instance
# state, but the per-connection fd flows through closure capture,
# not the earlier `Tep::APP.pending_*` stash + pause(0) handoff.
module Tep
  class Server
    class Scheduled
      # Max bytes accepted from a single request's start-line +
      # headers. Bigger requests get 413; matches the blocking
      # server's SPHTTP_BUFSIZE cap (64 KiB).
      MAX_REQUEST_BYTES = 65535

      # Idle keep-alive timeout between requests on the same
      # connection. 30s matches nginx; bump from app code as needed.
      KEEPALIVE_TIMEOUT = 30

      # Slow-headers DoS guard.
      HEADER_READ_TIMEOUT = 10

      attr_accessor :app

      def initialize(app)
        @app = app
      end

      def run(port, workers, quiet)
        sfd = Sock.sphttp_listen(port, workers > 1 ? 1 : 0)
        if sfd < 0
          # Loud + nonzero exit: the one plausible cause on a sane host
          # is another process already bound to the port, and silently
          # returning here (the caller discards the code) made the
          # binary look like it started and immediately vanished.
          $stderr.puts "tep: cannot bind to port " + port.to_s +
                       " (already in use?)"
          exit(1)
        end
        Sock.sphttp_set_nonblock(sfd)
        if !quiet
          # Same banner shape as the prefork Tep::Server, printed only
          # after a successful bind so startup and bind-failure are
          # distinguishable at a glance. Explicit flush: when stdout is
          # redirected (not a TTY) C stdio block-buffers, and a banner
          # that only shows up at exit is no banner at all.
          puts "[tep " + Tep::VERSION + "] listening on http://0.0.0.0:" +
               port.to_s + " (workers=" + workers.to_s + ")"
          $stdout.flush
        end

        # Install SIGTERM/SIGINT handlers BEFORE fork so children
        # inherit them; accept_loop checks the term flag once per
        # second and runs Tep.on_shutdown (run_end + future hooks).
        Sock.sphttp_install_term_handlers

        if workers > 1
          i = 0
          while i < workers
            pid = Sock.sphttp_fork
            if pid == 0
              Tep::Server::Scheduled.run_worker(sfd)
              Sock.sphttp_exit(0)
            end
            i += 1
          end
          # Reap children until none remain. After all workers exit,
          # emit the single aggregated run_end (see #128 / Tep::Events
          # #run_end_aggregated).
          loop do
            gone = Sock.sphttp_wait_any
            if gone < 0
              break
            end
          end
          if Sock.sphttp_shutdown_requested != 0
            Tep.on_shutdown
          end
        else
          Tep::Server::Scheduled.run_worker(sfd)
          # Single-process: this IS the parent; emit run_end here.
          if Sock.sphttp_shutdown_requested != 0
            Tep.on_shutdown
          end
        end
        0
      end

      # Spawn the accept fiber + pump the scheduler. Called inside
      # each prefork child. Loops directly on `tick` rather than
      # `run_until_empty` because the accept fiber parks on io_wait
      # indefinitely -- run_until_empty bails when no fiber is ready
      # to resume THIS pass; we need to keep polling so parked
      # accept-on-sfd fibers get woken when a connection arrives.
      def self.run_worker(sfd)
        f = Fiber.new { Tep::Server::Scheduled.accept_loop(sfd) }
        Tep::Scheduler.spawn_fiber(f)
        while Tep::Scheduler.alive_count > 0
          Tep::Scheduler.tick(1000)
        end
        0
      end

      # Accept loop. Each accepted connection becomes its own fiber
      # that closes over the just-accepted `client` fd.
      def self.accept_loop(sfd)
        while true
          # SIGTERM/SIGINT: sphttp's term flag is set by the signal
          # handler; check before parking on io_wait so we don't sleep
          # past a shutdown request. The 1s io_wait timeout below
          # bounds the sleep-side latency. The parent (or this same
          # process for workers=1) emits the aggregated run_end after
          # all workers exit (#128).
          if Sock.sphttp_shutdown_requested != 0
            break
          end
          # Bounded wait so the flag check above runs once per second
          # even when traffic is idle (was -1 = wait forever).
          ready = Tep::Scheduler.io_wait(sfd, Tep::Scheduler::READ, 1)
          if ready == 0
            next
          end
          client = Sock.sphttp_accept_nb(sfd)
          if client < 0
            next
          end
          Sock.sphttp_set_nonblock(client)
          conn = Fiber.new { Tep::Server::Scheduled.handle_connection(client) }
          Tep::Scheduler.spawn_fiber(conn)
        end
      end

      # Per-connection lifecycle. Per-request work lives in handle_one so
      # each keep-alive iteration gets its own SP_GC_SAVE/SP_GC_RESTORE
      # scope. Inline in this while loop, the per-request roots (blob, req,
      # res, and everything dispatch/render allocates) would hoist to this
      # function's long-lived scope and pile up across the connection's
      # requests -- defeating the young-gen GC and racing the heap threshold
      # upward (idle 2 MB -> multi-GB under sustained keep-alive load).
      # Mirrors the Tep::Server#handle_one fix (210a5f6) for the prefork
      # server; this Scheduled server is the one the blog actually runs.
      def self.handle_connection(client)
        keep_going = true
        while keep_going
          keep_going = Tep::Server::Scheduled.handle_one(client)
        end
        Sock.sphttp_close(client)
        0
      end

      # Process exactly one request on `client`. Returns true to keep the
      # connection open for the next keep-alive request, false to close.
      def self.handle_one(client)
        blob = Tep::Server::Scheduled.read_request_blob(client, KEEPALIVE_TIMEOUT)
        if blob.length == 0
          return false
        end
        req = Parser.parse(blob)
        if req == nil
          Tep::Server::Scheduled.send_simple(client, 400, "bad request")
          return false
        end

        req.consume_body_via_scheduler(client)

        res = Response.new
        Tep::APP.dispatch(req, res)

        # Streaming responses use chunked Connection: close (same
        # simplification as the prefork server) -- force the keep-alive
        # loop to end after this response so the stream's terminator isn't
        # followed by a stale read on the same fd.
        keep_alive = req.keep_alive? && !res.halted_close? && !res.streaming
        Tep::Server::Scheduled.write_response(client, req, res, keep_alive)
        keep_alive
      end

      # Non-blocking request reader. Returns the accumulated blob
      # once "\r\n\r\n" is seen, or "" on timeout / EOF / oversize.
      def self.read_request_blob(fd, timeout_seconds)
        buf = ""
        deadline = Time.now.to_i + timeout_seconds
        while buf.length < MAX_REQUEST_BYTES
          remaining = deadline - Time.now.to_i
          if remaining <= 0
            return ""
          end
          ready = Tep::Scheduler.io_wait(fd, Tep::Scheduler::READ, remaining)
          if ready == 0
            return ""
          end
          chunk = Sock.sphttp_recv_some(fd, 4096)
          if chunk.length == 0
            return ""
          end
          buf << chunk
          if buf.length >= 4 && buf.include?("\r\n\r\n")
            return buf
          end
        end
        ""
      end

      # Body-shape mirror of Tep::Server#write_response. Lifted into
      # a cmeth so the connection fiber can call it without a captured
      # `self`.
      def self.write_response(client, req, res, keep_alive)
        # WebSocket upgrade branch. Set by res.start_websocket in the
        # user's handler after a successful Handshake.check. Writes
        # the 101 Switching Protocols head, then assigns the client
        # fd onto the driver and runs the recv loop. The recv loop
        # returns when the connection closes (peer EOF, idle timeout,
        # or a CLOSE frame round-trip). After return, the caller's
        # handle_connection closes the fd as usual.
        if res.upgrading_ws
          head = Tep::WebSocket::Handshake.build_response(
            res.ws_accept_key, res.ws_driver.subprotocol)
          Sock.sphttp_write_str(client, head)
          res.ws_driver.set_fd(client)
          conn = Tep::WebSocket::Connection.new(res.ws_driver)
          conn.run
          return 0
        end

        # Streaming branch -- cooperative mirror of Tep::Server's
        # streaming path (server.rb). Set by res.start_stream(streamer)
        # in the handler. Writes a chunked-encoding head immediately,
        # hands a Tep::Stream writer to the user's Streamer#pump, then
        # emits the end-of-stream terminator. pump runs cooperatively:
        # it parks on Tep::Scheduler.io_wait between writes (e.g. the
        # proxy streamer waits on the upstream fd), so other fibers keep
        # running while this stream is in flight. Connection: close --
        # chunked keep-alive is legal but we keep it simple, matching
        # the prefork server.
        if res.streaming
          res.headers["Transfer-Encoding"] = "chunked"
          if !res.headers.key?("Content-Type")
            res.headers["Content-Type"] = "text/event-stream"
          end
          reason = Tep.reason(res.status)
          head = req.http_version + " " + res.status.to_s + " " + reason + "\r\n"
          res.headers.each do |k, v|
            head << k + ": " + v + "\r\n"
          end
          res.set_cookies.each do |line|
            head << "Set-Cookie: " + line + "\r\n"
          end
          head << "Connection: close\r\n\r\n"
          Sock.sphttp_write_str(client, head)
          out = Tep::Stream.new(client)
          res.streamer.pump(out)
          Sock.sphttp_write_chunk_end(client)
          return 0
        end

        # Default Content-Type for inline-body responses. Matches
        # Tep::Server#send; without it, the Security::Headers nosniff
        # default leaves the browser refusing to interpret an erb
        # response as HTML.
        if res.file_path.length == 0 && res.body.length > 0 && !res.headers.key?("Content-Type")
          res.headers["Content-Type"] = "text/html; charset=utf-8"
        end
        reason = Tep.reason(res.status)
        head = req.http_version + " " + res.status.to_s + " " + reason + "\r\n"
        res.headers.each do |k, v|
          head << k + ": " + v + "\r\n"
        end
        res.set_cookies.each do |line|
          head << "Set-Cookie: " + line + "\r\n"
        end
        if keep_alive
          head << "Connection: keep-alive\r\n"
        else
          head << "Connection: close\r\n"
        end
        if res.file_path.length > 0
          fs = Sock.sphttp_filesize(res.file_path)
          head << "Content-Length: " + fs.to_s + "\r\n\r\n"
          Sock.sphttp_write_str(client, head)
          Sock.sphttp_sendfile(client, res.file_path)
        else
          head << "Content-Length: " + res.body.length.to_s + "\r\n\r\n"
          Sock.sphttp_write_str(client, head)
          if res.body.length > 0
            Sock.sphttp_write_str(client, res.body)
          end
        end
        0
      end

      def self.send_simple(client, status, msg)
        reason = Tep.reason(status)
        head = "HTTP/1.0 " + status.to_s + " " + reason + "\r\n" +
               "Content-Length: " + msg.length.to_s + "\r\n" +
               "Connection: close\r\n\r\n" + msg
        Sock.sphttp_write_str(client, head)
        0
      end
    end
  end
end
