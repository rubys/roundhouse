# Tep::Server -- accept loop. Single-threaded inside one process;
# the perf model is "fork N workers, each runs its own Server"
# (-w workers) using SO_REUSEPORT so the kernel load-balances.
module Tep
  def self.reason(status)
    if status == 200; return "OK"; end
    if status == 201; return "Created"; end
    if status == 204; return "No Content"; end
    if status == 301; return "Moved Permanently"; end
    if status == 302; return "Found"; end
    if status == 304; return "Not Modified"; end
    if status == 400; return "Bad Request"; end
    if status == 401; return "Unauthorized"; end
    if status == 403; return "Forbidden"; end
    if status == 404; return "Not Found"; end
    if status == 500; return "Internal Server Error"; end
    "OK"
  end

  class Server
    attr_accessor :app

    def initialize(app)
      @app = app
    end

    def run(port, workers, quiet)
      if !quiet
        puts "[tep " + VERSION + "] listening on http://0.0.0.0:" + port.to_s +
             " (workers=" + workers.to_s + ")"
      end

      if workers <= 1
        sfd = Sock.sphttp_listen(port, 0)
        if sfd < 0
          $stderr.puts "tep: cannot bind to port " + port.to_s +
                       " (already in use?)"
          exit(1)
        end
        worker_loop(sfd)
        return
      end

      # Pre-fork. Each child opens its own SO_REUSEPORT listener so
      # the kernel load-balances accept() across workers.
      i = 0
      while i < workers
        pid = Sock.sphttp_fork
        if pid == 0
          sfd = Sock.sphttp_listen(port, 1)
          if sfd < 0
            puts "tep: worker " + Sock.sphttp_getpid.to_s + " bind failed"
            exit(1)
          end
          worker_loop(sfd)
          exit(0)
        end
        i += 1
      end
      # Parent: reap forever.
      loop do
        gone = Sock.sphttp_wait_any
        if gone < 0
          break
        end
      end
    end

    def worker_loop(sfd)
      loop do
        client = Sock.sphttp_accept(sfd)
        if client < 0
          next
        end
        handle_connection(client)
      end
    end

    # Keep-alive loop on a single accepted connection.
    # Per-request work is in handle_one so each iteration gets its own
    # SP_GC_SAVE/RESTORE scope — without that, transient roots from
    # earlier iterations pile up in the global root set inside the
    # surrounding while loop and prevent the young-gen GC from
    # reclaiming the previous request's allocations.
    def handle_connection(client)
      keep_going = true
      while keep_going
        keep_going = handle_one(client)
      end
      Sock.sphttp_close(client)
    end

    # Process exactly one request on `client`. Returns true if the
    # connection should remain open for the next request (keep-alive)
    # or false if it should close.
    # Blocking request reader: accumulate recv_some until the header
    # terminator "\r\n\r\n" (or EOF / 64 KiB cap). The prefork server's
    # client fd is blocking, so each recv parks the worker until bytes
    # arrive — no scheduler. Replaces the C sphttp_read_request +
    # request_buf (sphttp.c retired — matz/spinel#1466). `+` is binary-safe.
    def read_request_blocking(client)
      buf = +""
      while buf.length < 65535
        chunk = Sock.sp_net_recv_some(client, 4096)
        if chunk.length == 0
          return ""
        end
        buf = buf + chunk
        if buf.length >= 4 && buf.include?("\r\n\r\n")
          return buf
        end
      end
      ""
    end

    def handle_one(client)
      blob = read_request_blocking(client)
      return false if blob.length == 0
      req = Parser.parse(blob)
      if req == nil
        send_simple(client, 400, "bad request")
        return false
      end
      req.consume_body(client)
      res = Response.new
      @app.dispatch(req, res)
      keep_alive = req.keep_alive? && !res.halted_close?
      write_response(client, req, res, keep_alive)
      keep_alive
    end

    def write_response(client, req, res, keep_alive)
      if res.streaming
        # Chunked-encoding stream. Send headers immediately, hand a
        # Stream writer to the user's Streamer.pump, emit terminator.
        # Connection is closed afterwards (keep-alive + chunked is
        # technically legal but we keep things simple).
        res.headers["Transfer-Encoding"] = "chunked"
        res.headers["Connection"] = "close"
        if !res.headers.key?("Content-Type")
          res.headers["Content-Type"] = "text/event-stream"
        end
        head = build_head(req, res)
        Sock.sphttp_write_str(client, head)
        out = Stream.new(client)
        res.streamer.pump(out)
        Sock.sphttp_write_chunk_end(client)
        return
      end

      if res.file_path.length > 0
        # send_file path -- compute size, emit headers, then stream.
        sz = Sock.sphttp_filesize(res.file_path)
        if sz < 0
          send_simple(client, 404, "file not found")
          return
        end
        res.headers["Content-Length"] = sz.to_s
        if !res.headers.key?("Content-Type")
          res.headers["Content-Type"] = "application/octet-stream"
        end
        if keep_alive
          res.headers["Connection"] = "keep-alive"
        else
          res.headers["Connection"] = "close"
        end
        head = build_head(req, res)
        Sock.sphttp_write_str(client, head)
        Sock.sphttp_sendfile(client, res.file_path)
        return
      end

      if res.body.length > 0 && !res.headers.key?("Content-Type")
        res.headers["Content-Type"] = "text/html; charset=utf-8"
      end
      res.headers["Content-Length"] = res.body.length.to_s
      if keep_alive
        res.headers["Connection"] = "keep-alive"
      else
        res.headers["Connection"] = "close"
      end

      head = build_head(req, res)
      Sock.sphttp_write_str(client, head)
      if res.body.length > 0
        Sock.sphttp_write_str(client, res.body)
      end
    end

    def build_head(req, res)
      reason = Tep.reason(res.status)
      head = req.http_version + " " + res.status.to_s + " " + reason + "\r\n"
      res.headers.each do |k, v|
        head << k + ": " + v + "\r\n"
      end
      # Set-Cookie can repeat; emit each on its own line.
      ci = 0
      while ci < res.set_cookies.length
        head << "Set-Cookie: " + res.set_cookies[ci] + "\r\n"
        ci += 1
      end
      head + "\r\n"
    end

    def send_simple(client, status, msg)
      reason = Tep.reason(status)
      body = "<h1>" + status.to_s + " " + reason + "</h1><p>" + msg + "</p>\n"
      head = "HTTP/1.0 " + status.to_s + " " + reason + "\r\n" +
             "Content-Type: text/html; charset=utf-8\r\n" +
             "Content-Length: " + body.length.to_s + "\r\n" +
             "Connection: close\r\n\r\n"
      Sock.sphttp_write_str(client, head + body)
    end
  end
end
