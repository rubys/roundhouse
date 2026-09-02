# Tep::Server::Threaded -- a green thread per connection.
#
# The connection is a `Thread`. spinel's Thread is a green thread on an
# M:N scheduler: every `recv` and `write` in sp_net parks the thread on
# EAGAIN and frees its OS worker, `sleep` parks, and the timed waits
# below park with a deadline (matz/spinel#4262). So a held-open
# WebSocket costs one small stack, a slow subscriber stalls only its own
# writer, and the workers run other connections' Ruby in parallel.
#
# This replaces Tep::Server::Scheduled, a fiber-per-connection server
# over a Ruby poll loop. That design had a wall in C below it -- a
# blocking write inside the net layer that no Ruby scheduler could see
# -- and a poll set rebuilt every tick. Both now belong to the runtime's
# monitor thread, which is the point of building on `Thread`.
#
# Client fds stay NON-BLOCKING: sp_net parks only on EAGAIN, so a
# blocking fd would pin the OS worker in the syscall instead. Timed
# waits go through one `IO.for_fd` wrapper per connection (a dup, closed
# with the connection), because a raw fd has no `wait_readable`.
#
# ONE OS WORKER, FOR NOW -- every launcher sets SPINEL_WORKERS=1 (the
# campfire archive's README and e2e config, scripts/campfire-compare,
# campfire-cable-walk, campfire-queries, e2e, compare, bench). This is
# the evidence, 2026-09-02, spinel 3428632f, 16-core Linux/gcc: with the
# autodetected worker count the binary died under a browser's parallel
# load of the room page in 4 of 7 runs -- gdb: SIGSEGV in
# sp_StrArray_scan inside sp_gc_mark_drain, a stop-the-world mark
# reaching an Array[String] whose element was already freed; in one run
# it did not die and a cable subscribe quietly never confirmed instead.
# With SPINEL_WORKERS=1, 9 of 9 runs green. A browser-free burst (30
# rounds of 168 unauthenticated GETs, 48-way parallel) does not
# reproduce it, so the signed-in page render is part of the shape. Not
# yet known: whether the freed element is shared state this server
# still leaves unlocked or a root gap in the runtime's multi-worker
# collector. Until that is known, one worker is the validated
# configuration -- the semantics the fiber server had, every I/O parking
# -- and matz/spinel#4266 asks for a program-level cap so the binary can
# pin itself instead of every launcher. Lift the pin with evidence, not
# by deleting it.
module Tep
  class Server
    class Threaded
      # Max bytes accepted from a single request's start-line + headers.
      # Bigger requests get 413; matches the blocking server's
      # SPHTTP_BUFSIZE cap (64 KiB).
      MAX_REQUEST_BYTES = 65535

      # Idle keep-alive timeout between requests on the same connection.
      KEEPALIVE_TIMEOUT = 30

      attr_accessor :app

      def initialize(app)
        @app = app
      end

      def run(port, workers, quiet)
        sfd = Sock.sphttp_listen(port, workers > 1 ? 1 : 0)
        if sfd < 0
          $stderr.puts "tep: cannot bind to port " + port.to_s +
                       " (already in use?)"
          exit(1)
        end
        if !quiet
          puts "[tep " + Tep::VERSION + "] listening on http://0.0.0.0:" +
               port.to_s + " (workers=" + workers.to_s + ")"
          $stdout.flush
        end

        # Install SIGTERM/SIGINT handlers BEFORE fork so children inherit
        # them; the accept loop checks the term flag after every accept.
        Sock.sphttp_install_term_handlers

        # `--workers N` still preforks: each child is a threaded server
        # of its own. One process is the shape the binary is measured
        # in; the option stays for the operator who wants processes.
        if workers > 1
          i = 0
          while i < workers
            pid = Sock.sphttp_fork
            if pid == 0
              Tep::Server::Threaded.run_worker(sfd)
              Sock.sphttp_exit(0)   # same reason as the single-process exit below
            end
            i += 1
          end
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
          Tep::Server::Threaded.run_worker(sfd)
          if Sock.sphttp_shutdown_requested != 0
            Tep.on_shutdown
          end
          # Exit outright: spinel runs every remaining green thread to
          # completion when main returns, and the connection threads are
          # parked on sockets that may never speak again (a keep-alive
          # waits 30s, an idle WebSocket 300s). A server asked to stop
          # stops.
          Sock.sphttp_exit(0)
        end
        0
      end

      # The accept loop, on the calling thread. A TIMED wait on the listen
      # socket, one second at a time, so SIGTERM/SIGINT is noticed within
      # a second even when no connection arrives: the signal handler only
      # sets a flag, and a thread parked in a plain `accept` is never
      # woken to read it -- the first threaded build ignored SIGTERM until
      # the next connection came in. Then a non-blocking accept, which
      # answers -1 for a spurious wake and is retried.
      def self.run_worker(sfd)
        Sock.sphttp_set_nonblock(sfd)
        lio = IO.for_fd(sfd, autoclose: false)
        while true
          if Sock.sphttp_shutdown_requested != 0
            break
          end
          ready = lio.wait_readable(1)
          if ready.nil?
            next
          end
          client = Sock.sphttp_accept_nb(sfd)
          if client < 0
            next
          end
          Sock.sphttp_set_nonblock(client)
          Thread.new(client) do |c|
            Tep::Server::Threaded.handle_connection(c)
          end
        end
        lio.close
        0
      end

      # Per-connection lifecycle: one wrapper for the timed waits, the
      # keep-alive loop, then both the wrapper's dup and the fd close.
      # Per-request work lives in handle_one so each keep-alive iteration
      # gets its own GC scope (see Tep::Server#handle_one, 210a5f6).
      def self.handle_connection(client)
        io = IO.for_fd(client, autoclose: false)
        keep_going = true
        while keep_going
          keep_going = Tep::Server::Threaded.handle_one(client, io)
        end
        io.close
        Sock.sphttp_close(client)
        0
      end

      # Process exactly one request on `client`. Returns true to keep the
      # connection open for the next keep-alive request, false to close.
      def self.handle_one(client, io)
        blob = Tep::Server::Threaded.read_request_blob(client, io, KEEPALIVE_TIMEOUT)
        if blob.length == 0
          return false
        end
        req = Parser.parse(blob)
        if req == nil
          Tep::Server::Threaded.send_simple(client, 400, "bad request")
          return false
        end

        req.consume_body_via_io(io, client)

        res = Response.new
        begin
          Tep::APP.dispatch(req, res)
        # Both names, as in the blocking server: a stubbed gem facade
        # raises NotImplementedError, a ScriptError, which a bare
        # `rescue` does not catch.
        rescue StandardError, ScriptError => e
          # One request's failure is not the connection's, and certainly
          # not the process's: a thread that unwound here would take
          # only itself down, but the client deserves the 500.
          Tep.log_dispatch_error(req.verb, req.path, e)
          Tep::Server::Threaded.send_simple(client, 500, "internal server error")
          return false
        end

        # Streaming responses use chunked Connection: close (same
        # simplification as the prefork server).
        keep_alive = req.keep_alive? && !res.halted_close? && !res.streaming
        Tep::Server::Threaded.write_response(client, io, req, res, keep_alive)
        keep_alive
      end

      # Request reader. Returns the accumulated blob once "\r\n\r\n" is
      # seen, or "" on timeout / EOF / oversize. The timed wait parks the
      # thread; a peer that closes wakes it (EOF reads as zero bytes).
      def self.read_request_blob(fd, io, timeout_seconds)
        buf = +""
        deadline = Time.now.to_i + timeout_seconds
        while buf.length < MAX_REQUEST_BYTES
          remaining = deadline - Time.now.to_i
          if remaining <= 0
            return ""
          end
          ready = io.wait_readable(remaining)
          if ready.nil?
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

      # Body-shape mirror of Tep::Server#write_response.
      def self.write_response(client, io, req, res, keep_alive)
        # WebSocket upgrade branch. Set by res.start_websocket in the
        # user's handler after a successful Handshake.check. Writes the
        # 101 Switching Protocols head, then hands the fd (and this
        # connection's wait wrapper) to the driver and runs the recv
        # loop, which returns when the connection closes.
        if res.upgrading_ws
          head = Tep::WebSocket::Handshake.build_response(
            res.ws_accept_key, res.ws_driver.subprotocol)
          Sock.sphttp_write_str(client, head)
          res.ws_driver.set_fd(client)
          conn = Tep::WebSocket::Connection.new(res.ws_driver, io)
          conn.run
          return 0
        end

        # Streaming branch -- chunked, Connection: close.
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

        # Default Content-Type for inline-body responses.
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
          # BYTES, both times: `length` counts characters, and
          # `write_str` crosses the FFI as a NUL-terminated C string.
          head << "Content-Length: " + res.body.bytesize.to_s + "\r\n\r\n"
          Sock.sphttp_write_str(client, head)
          if res.body.bytesize > 0
            Sock.sphttp_write_bytes(client, res.body, res.body.bytesize)
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
