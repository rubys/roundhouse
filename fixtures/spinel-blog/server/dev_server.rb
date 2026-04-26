#!/usr/bin/env ruby
# spinel-blog dev server. Pure-Ruby TCP listener that:
#   - serves static assets from `static/`         (GET /assets/*)
#   - terminates WebSocket connections            (GET /cable, Upgrade)
#   - dispatches everything else to main.rb       (CGI fork+exec)
#   - watches BROADCAST_DIR for `.frag` files     (background thread)
#     and forwards each fragment to subscribed Turbo clients over WS
#
# Action Cable subprotocol implemented per Hotwire's expectations:
# - Sec-WebSocket-Protocol: actioncable-v1-json (echoed in handshake)
# - {"type":"welcome"} on connect
# - {"type":"ping","message":<unix>} every 3s
# - {"type":"confirm_subscription","identifier":"..."} after subscribe
# - {"type":"message","identifier":"...","message":"<turbo-stream>..."}
#   for each broadcast
#
# Stream-name unsigning skipped: the `signed_stream_name` field in the
# subscribe identifier is treated as a literal stream name. Demo-grade;
# matches the view helper's emit shape (no signing on the way out).

require "socket"
require "json"
require "digest/sha1"
require "base64"
require "fileutils"
require "thread"

module DevServer
  PORT             = (ENV["PORT"] || "3000").to_i
  ROOT             = File.expand_path("..", __dir__)
  STATIC_DIR       = File.join(ROOT, "static")
  BROADCAST_DIR    = ENV["BROADCAST_DIR"] || File.join(ROOT, "tmp", "broadcasts")
  MAIN_RB          = File.join(ROOT, "main.rb")
  WS_GUID          = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
  PING_INTERVAL    = 3.0
  WATCHER_INTERVAL = 0.1

  # stream_name → Array of [socket, identifier_str, write_mutex]
  @subscribers       = {}
  @subscribers_mutex = Mutex.new

  module_function

  def run
    FileUtils.mkdir_p(BROADCAST_DIR)
    ENV["BROADCAST_DIR"] = BROADCAST_DIR
    Thread.abort_on_exception = false

    server = TCPServer.new("0.0.0.0", PORT)
    puts "spinel-blog dev server"
    puts "  http://localhost:#{PORT}"
    puts "  static  : #{STATIC_DIR}"
    puts "  cable   : ws://localhost:#{PORT}/cable"
    puts "  watcher : #{BROADCAST_DIR}"
    puts "  cgi     : ruby #{MAIN_RB}"
    puts "  Ctrl-C to quit"

    Thread.new { broadcast_watcher_loop }

    loop do
      client = server.accept
      Thread.new { handle_client(client) }
    end
  rescue Interrupt
    puts "\nshutting down."
  end

  # ── HTTP request reading + routing ────────────────────────────

  def handle_client(socket)
    request = read_http_request(socket)
    return if request.nil?

    if request[:upgrade] == "websocket" && request[:path] == "/cable"
      handle_websocket(socket, request)
    elsif request[:path].start_with?("/assets/")
      handle_static(socket, request)
    else
      handle_cgi(socket, request)
    end
  rescue => e
    warn "[dev-server] #{e.class}: #{e.message}"
  ensure
    socket.close rescue nil
  end

  def read_http_request(socket)
    request_line = socket.gets("\r\n")
    return nil if request_line.nil?
    method, target, _version = request_line.strip.split(" ", 3)
    return nil if method.nil? || target.nil?
    path, query = target.split("?", 2)

    headers = {}
    while (line = socket.gets("\r\n"))
      line = line.chomp
      break if line.empty?
      key, val = line.split(":", 2)
      headers[key.strip.downcase] = val.to_s.strip if key
    end

    body = "".b
    if (cl = headers["content-length"]&.to_i) && cl > 0
      body = socket.read(cl).to_s
    end

    {
      method:             method,
      path:               path || "/",
      query:              query || "",
      headers:            headers,
      body:               body,
      upgrade:            headers["upgrade"]&.downcase,
      websocket_key:      headers["sec-websocket-key"],
      websocket_protocol: headers["sec-websocket-protocol"],
    }
  end

  # ── Static files (GET /assets/*) ──────────────────────────────

  CONTENT_TYPES = {
    ".css"  => "text/css",
    ".js"   => "application/javascript",
    ".json" => "application/json",
    ".png"  => "image/png",
    ".svg"  => "image/svg+xml",
    ".ico"  => "image/x-icon",
  }.freeze

  def handle_static(socket, request)
    rel  = request[:path].sub(%r{\A/assets/}, "")
    full = File.expand_path(rel, STATIC_DIR)
    if !full.start_with?(STATIC_DIR + "/") || !File.file?(full)
      respond(socket, 404, "Not Found", "text/plain", "Not Found")
      return
    end
    body  = File.binread(full)
    ctype = CONTENT_TYPES[File.extname(full)] || "application/octet-stream"
    respond(socket, 200, "OK", ctype, body, "Cache-Control" => "public, max-age=300")
  end

  def respond(socket, status, reason, ctype, body, extra_headers = {})
    socket.write("HTTP/1.1 #{status} #{reason}\r\n")
    socket.write("Content-Type: #{ctype}\r\n")
    socket.write("Content-Length: #{body.bytesize}\r\n")
    extra_headers.each { |k, v| socket.write("#{k}: #{v}\r\n") }
    socket.write("Connection: close\r\n\r\n")
    socket.write(body)
  end

  # ── CGI dispatch (everything else) ────────────────────────────

  def handle_cgi(socket, request)
    env = {
      "REQUEST_METHOD" => request[:method],
      "PATH_INFO"      => request[:path],
      "QUERY_STRING"   => request[:query],
      "CONTENT_LENGTH" => request[:body].bytesize.to_s,
      "CONTENT_TYPE"   => request[:headers]["content-type"] || "",
      "HTTP_COOKIE"    => request[:headers]["cookie"]       || "",
      "HTTP_HOST"      => request[:headers]["host"]         || "",
      "BROADCAST_DIR"  => BROADCAST_DIR,
      "BLOG_DB"        => ENV["BLOG_DB"]                    || "tmp/blog.sqlite3",
    }
    cgi_output = IO.popen(env, ["ruby", MAIN_RB], "r+b") do |pipe|
      pipe.write(request[:body]) if request[:body].bytesize > 0
      pipe.close_write
      pipe.read
    end
    forward_cgi_response(socket, cgi_output)
  end

  REASON_PHRASES = {
    200 => "OK", 201 => "Created", 204 => "No Content",
    301 => "Moved Permanently", 302 => "Found", 303 => "See Other",
    304 => "Not Modified",
    400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden",
    404 => "Not Found", 422 => "Unprocessable Entity",
    500 => "Internal Server Error",
  }.freeze

  def forward_cgi_response(socket, cgi_output)
    sep   = cgi_output.index("\r\n\r\n")
    head  = sep ? cgi_output[0...sep] : cgi_output
    body  = sep ? cgi_output[(sep + 4)..]     : ""

    status_code   = 200
    status_reason = "OK"
    out_headers   = []
    head.split("\r\n").each do |line|
      if line =~ /\AStatus:\s*(\d+)\s*(.*)\z/
        status_code   = $1.to_i
        status_reason = $2.empty? ? (REASON_PHRASES[status_code] || "OK") : $2
      else
        out_headers << line
      end
    end

    socket.write("HTTP/1.1 #{status_code} #{status_reason}\r\n")
    out_headers.each { |h| socket.write("#{h}\r\n") }
    socket.write("Content-Length: #{body.bytesize}\r\n")
    socket.write("Connection: close\r\n\r\n")
    socket.write(body)
  end

  # ── WebSocket handshake + per-connection loop ─────────────────

  def handle_websocket(socket, request)
    key = request[:websocket_key].to_s
    if key.empty?
      respond(socket, 400, "Bad Request", "text/plain", "Missing Sec-WebSocket-Key")
      return
    end
    accept = Base64.strict_encode64(Digest::SHA1.digest(key + WS_GUID))

    socket.write("HTTP/1.1 101 Switching Protocols\r\n")
    socket.write("Upgrade: websocket\r\n")
    socket.write("Connection: Upgrade\r\n")
    socket.write("Sec-WebSocket-Accept: #{accept}\r\n")
    if request[:websocket_protocol].to_s.split(",").map(&:strip).include?("actioncable-v1-json")
      socket.write("Sec-WebSocket-Protocol: actioncable-v1-json\r\n")
    end
    socket.write("\r\n")

    write_mutex   = Mutex.new
    subscriptions = []  # Array of [stream_name, entry] we registered

    safe_send = ->(payload) {
      begin
        ws_send_text(socket, payload, write_mutex)
        true
      rescue Errno::EPIPE, Errno::ECONNRESET, IOError
        false
      end
    }

    safe_send.call(JSON.generate(type: "welcome"))

    ping_thread = Thread.new do
      loop do
        sleep PING_INTERVAL
        break if socket.closed?
        break unless safe_send.call(JSON.generate(type: "ping", message: Time.now.to_i))
      end
    end

    begin
      loop do
        msg = ws_read_frame(socket)
        break if msg.nil?  # close frame or EOF
        next  if msg.empty?
        payload = JSON.parse(msg) rescue nil
        next unless payload.is_a?(Hash) && payload["command"] == "subscribe"
        identifier_str = payload["identifier"]
        next unless identifier_str.is_a?(String)
        identifier = JSON.parse(identifier_str) rescue nil
        next unless identifier.is_a?(Hash)
        stream = identifier["signed_stream_name"]
        next unless stream.is_a?(String) && !stream.empty?

        entry = [socket, identifier_str, write_mutex]
        @subscribers_mutex.synchronize do
          @subscribers[stream] ||= []
          @subscribers[stream] << entry
        end
        subscriptions << [stream, entry]

        safe_send.call(JSON.generate(type: "confirm_subscription", identifier: identifier_str))
      end
    ensure
      ping_thread.kill rescue nil
      @subscribers_mutex.synchronize do
        subscriptions.each do |stream, entry|
          subs = @subscribers[stream]
          next if subs.nil?
          subs.delete(entry)
          @subscribers.delete(stream) if subs.empty?
        end
      end
    end
  end

  # ── WebSocket framing (RFC 6455) ──────────────────────────────

  # Send a single text frame (FIN=1, opcode=0x01). Server frames are
  # never masked. Mutex argument serializes writes from the read loop,
  # the ping thread, and the broadcast watcher.
  def ws_send_text(socket, payload, mutex = nil)
    bytes = payload.bytesize
    frame = "".b
    frame << 0x81  # 1000_0001 — FIN + text opcode
    if bytes < 126
      frame << bytes
    elsif bytes < 65_536
      frame << 126
      frame << [bytes].pack("n")
    else
      frame << 127
      frame << [bytes].pack("Q>")
    end
    frame << payload.b
    if mutex
      mutex.synchronize { socket.write(frame) }
    else
      socket.write(frame)
    end
  end

  # Read a single frame from the client. Returns the unmasked text
  # payload, or nil on close/EOF, or "" for non-text/control frames
  # we choose to ignore. Client frames must be masked per RFC 6455.
  def ws_read_frame(socket)
    byte0_buf = socket.read(1)
    return nil if byte0_buf.nil? || byte0_buf.empty?
    byte0 = byte0_buf.bytes.first
    opcode = byte0 & 0x0F
    return nil if opcode == 0x08  # close

    byte1 = socket.read(1).bytes.first
    masked = (byte1 & 0x80) != 0
    length = byte1 & 0x7F
    if length == 126
      length = socket.read(2).unpack1("n")
    elsif length == 127
      length = socket.read(8).unpack1("Q>")
    end

    return "" unless masked  # protocol violation; skip silently
    mask    = socket.read(4).bytes
    payload = socket.read(length).bytes
    payload.each_with_index { |b, i| payload[i] = b ^ mask[i % 4] }

    case opcode
    when 0x01 then payload.pack("C*").force_encoding("UTF-8")
    when 0x09 then ""  # ping from client; ignore (we don't run a pong path)
    else ""
    end
  rescue EOFError, Errno::ECONNRESET, IOError
    nil
  end

  # ── Broadcast watcher ─────────────────────────────────────────

  # Polls BROADCAST_DIR every WATCHER_INTERVAL seconds. For each
  # `.frag` file: derives the stream name from the filename
  # (`<stream>__<ts>.frag`), reads the fragment HTML, fans out to
  # every subscriber on that stream, deletes the file. Atomic
  # write on the producer side (`.tmp` → rename) means we never
  # see a half-written file.
  def broadcast_watcher_loop
    loop do
      sleep WATCHER_INTERVAL
      Dir.glob(File.join(BROADCAST_DIR, "*.frag")).sort.each do |path|
        begin
          name     = File.basename(path, ".frag")
          stream   = name.split("__", 2).first
          fragment = File.read(path)
          File.delete(path)
          dispatch_to_subscribers(stream, fragment)
        rescue Errno::ENOENT
          # already consumed
        rescue => e
          warn "[watcher] #{e.class}: #{e.message}"
        end
      end
    end
  end

  def dispatch_to_subscribers(stream, fragment)
    snapshot = @subscribers_mutex.synchronize do
      (@subscribers[stream] || []).dup
    end
    snapshot.each do |socket, identifier, mutex|
      payload = JSON.generate(type: "message", identifier: identifier, message: fragment)
      begin
        ws_send_text(socket, payload, mutex)
      rescue Errno::EPIPE, Errno::ECONNRESET, IOError
        # Subscriber gone; per-connection cleanup runs in the WS
        # ensure block when its read loop notices the close.
      end
    end
  end
end

DevServer.run if __FILE__ == $PROGRAM_NAME
