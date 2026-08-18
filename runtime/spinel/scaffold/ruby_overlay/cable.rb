# Cable — Action-Cable-shape WebSocket endpoint for the CRuby target.
#
# ARCHITECTURE. One reactor thread owns every connection: a single
# NIO::Selector multiplexes all sockets, and that same loop's select
# timeout drives the shared heartbeat. Puma request threads never touch
# a socket, a driver, a monitor or a subscription table — they post a
# closure onto the reactor's queue and return immediately.
#
# This mirrors Action Cable's own layering (StreamEventLoop's single
# nio4r thread plus one server-wide BEAT_INTERVAL timer) rather than the
# thread-per-connection shape this file used to have. That shape cost
# 2N OS threads for N connections — one blocking on readpartial, one
# sleeping between pings — and, worse, fanned a broadcast out on the
# PUMA THREAD that triggered it: `POST /rooms/1/messages` into a room
# with 1,000 subscribers did 1,000 JSON encodings and 1,000 BLOCKING
# socket writes before it could return 200, so one wedged client stalled
# both the broadcast and the request behind it.
#
# The fan-out inversion is borrowed from the spinel target's cable.rb,
# which publishes one composed envelope to a stream topic instead of
# walking connection objects: `Registry.deliver` encodes the message
# once and reuses the frame across every subscriber sharing an
# identifier (for Turbo streams, all of them).
#
# Deliberate differences from Action Cable, and why:
#
#   * The heartbeat rides the selector's own select timeout rather than
#     a Concurrent::TimerTask. One less thread, one less dependency,
#     same 3-second cadence.
#   * There is no worker pool. Action Cable hands each inbound frame to
#     one because a channel action runs arbitrary app code and needs its
#     own AR connection. Nothing here dispatches to app code yet — a
#     subscribe is a hash write — so frames are handled inline on the
#     reactor thread. A worker pool is what this grows when channel
#     subscription dispatch lands.
#   * A connection whose outbound buffer passes MAX_BUFFER_BYTES is
#     closed. Action Cable buffers without a ceiling; here one client
#     that stops reading would otherwise be unbounded memory.
#
# THREADING CONTRACT, in one line: everything below runs on the reactor
# thread except `Reactor.post`, `Reactor.attach` and
# `Registry.broadcast`. That is why SUBS and the connection table carry
# no mutex — they have exactly one writer.
#
# Single-worker only. Clustered Puma (workers > 1) needs an inter-worker
# pubsub — Redis in campfire's own deployment — behind the same
# `Broadcasts.set_transport` seam this file plugs into.
require "nio"
require "websocket/driver"
require "json"
require "base64"

module Cable
  # Action Cable's browser client requires both of these: it refuses a
  # connection whose 101 doesn't echo a subprotocol it offered, and it
  # treats a gap in server pings (default ~6s) as a dead socket and
  # reconnects in a loop.
  PROTOCOLS = ["actioncable-v1-json"].freeze
  PING_INTERVAL = 3 # seconds — matches Action Cable's BEAT_INTERVAL

  # The single event loop. Owns the selector, the connection table and
  # the heartbeat deadline; serializes every mutation of both onto its
  # own thread via `post`.
  module Reactor
    START_MUTEX = Mutex.new

    @selector = nil
    @thread = nil
    @todo = Queue.new
    @connections = {}
    @wake_read = nil
    @wake_write = nil

    class << self
      # Run `block` on the reactor thread. Safe from any thread; this
      # and `attach` are the only cross-thread entry points.
      def post(&block)
        ensure_started
        @todo << block
        wakeup
        nil
      end

      # Hand a freshly hijacked connection to the loop. The driver is
      # NOT started here: `start` writes the 101 (and the welcome frame
      # its :open callback emits) into the connection's buffer, which
      # means it has to run after the monitor exists, on the reactor
      # thread, like every other driver call.
      def attach(conn)
        post do
          monitor = @selector.register(conn.socket, :r)
          monitor.value = conn
          conn.monitor = monitor
          @connections[conn.socket] = conn
          conn.start
        end
      end

      # Reactor thread only — called from Connection#close.
      def remove(conn)
        @connections.delete(conn.socket)
        begin
          @selector.deregister(conn.socket)
        rescue StandardError
          # already gone; the close path is best-effort by design
        end
        nil
      end

      def connection_count
        @connections.size
      end

      private

      def ensure_started
        return if @thread&.alive?

        START_MUTEX.synchronize do
          return if @thread&.alive?

          @selector = NIO::Selector.new
          # Self-pipe: lets `post` interrupt a blocking select so queued
          # work runs now rather than at the next heartbeat.
          @wake_read, @wake_write = IO.pipe
          @selector.register(@wake_read, :r).value = :wakeup

          @thread = Thread.new { run }
          @thread.name = "cable-reactor" if @thread.respond_to?(:name=)
        end
        nil
      end

      def wakeup
        @wake_write.write_nonblock(".", exception: false)
      rescue IOError, SystemCallError
        nil
      end

      def monotonic
        Process.clock_gettime(Process::CLOCK_MONOTONIC)
      end

      def run
        next_beat = monotonic + PING_INTERVAL

        loop do
          timeout = next_beat - monotonic
          timeout = 0 if timeout.negative?

          ready = @selector.select(timeout)
          drain_todo

          ready&.each do |monitor|
            if monitor.value == :wakeup
              drain_wakeup
            else
              service(monitor)
            end
          end

          if monotonic >= next_beat
            beat
            next_beat = monotonic + PING_INTERVAL
          end
        end
      end

      # One connection's readiness. Guarded per-connection: a socket
      # that raises on teardown must not take the loop — and therefore
      # every other connection — down with it.
      def service(monitor)
        conn = monitor.value
        conn.on_writable if monitor.writable? && !conn.closed?
        conn.on_readable if monitor.readable? && !conn.closed?
      rescue StandardError
        begin
          conn&.close
        rescue StandardError
          nil
        end
      end

      def drain_todo
        until @todo.empty?
          begin
            task = @todo.pop(true)
          rescue ThreadError
            break
          end
          begin
            task.call
          rescue StandardError
            nil
          end
        end
      end

      def drain_wakeup
        loop do
          byte = @wake_read.read_nonblock(1024, exception: false)
          break if byte == :wait_readable || byte.nil?
        end
      rescue IOError, SystemCallError
        nil
      end

      # The server-wide heartbeat: one timestamp, one payload string,
      # framed per connection (websocket-driver owns the framing, and
      # each driver holds its own write state). `values` rather than
      # `each_value` because a failed write closes, which mutates the
      # table mid-iteration.
      def beat
        payload = %({"type":"ping","message":#{Time.now.to_i}})
        @connections.values.each { |conn| conn.send_text(payload) }
        nil
      end
    end
  end

  # The `Broadcasts` transport: stream name → subscribed connections.
  # Reactor thread only (see the threading contract above), so no lock.
  module Registry
    SUBS = {}

    # Called from Puma request threads by `Broadcasts.record` after a
    # model commit. Returns as soon as the closure is queued — the
    # request never waits on a socket.
    def self.broadcast(stream, message)
      Reactor.post { Registry.deliver(stream, message) }
      nil
    end

    # Reactor thread. `message` is a rendered <turbo-stream> String from
    # `Broadcasts.record`, or an arbitrary Hash from
    # `ActionCable.server.broadcast` — `JSON.generate` gives the right
    # `message` value for either, and the Hash must stay a Hash so it
    # lands as a JSON object rather than a JSON-encoded string.
    #
    # Encoded ONCE per broadcast, then shared: the envelope varies only
    # by identifier, and every Turbo subscriber to a given stream sends
    # the same one, so the usual case composes a single frame no matter
    # how many connections receive it.
    def self.deliver(stream, message)
      conns = SUBS[stream]
      return nil if conns.nil? || conns.empty?

      message_json = JSON.generate(message)
      frames = {}

      conns.dup.each do |conn|
        identifier = conn.identifier_for(stream)
        next if identifier.nil?

        frame = frames[identifier] ||=
          %({"identifier":#{JSON.generate(identifier)},"message":#{message_json}})
        conn.send_text(frame)
      end
      nil
    end

    def self.subscribe(stream, conn)
      list = (SUBS[stream] ||= [])
      list << conn unless list.include?(conn)
      nil
    end

    def self.unsubscribe_all(conn)
      SUBS.each_value { |list| list.delete(conn) }
      SUBS.delete_if { |_stream, list| list.empty? }
      nil
    end
  end

  # Per-connection state: the hijacked socket, its websocket-driver, the
  # streams it subscribed to, and the bytes owed to it. Every method
  # here runs on the reactor thread.
  class Connection
    # A client that stops reading parks its frames here. Past this the
    # connection is dropped rather than allowed to grow without bound.
    MAX_BUFFER_BYTES = 4 * 1024 * 1024

    attr_reader :socket
    attr_accessor :monitor

    def initialize(env, socket)
      @socket = socket
      @buffer = +""
      @subscriptions = {}   # stream name → identifier JSON, echoed back
      @monitor = nil
      @closed = false

      @driver = WebSocket::Driver.rack(EnvAdapter.new(env, self), protocols: PROTOCOLS)
      @driver.on(:open)    { send_json({ "type" => "welcome" }) }
      @driver.on(:message) { |event| handle_message(event.data) }
      @driver.on(:close)   { close }
    end

    def closed?
      @closed
    end

    # Emits the 101 handshake, then the :open callback's welcome frame —
    # both into @buffer, which is why the monitor must already exist.
    def start
      @driver.start
    end

    def identifier_for(stream)
      @subscriptions[stream]
    end

    # websocket-driver's output sink (via EnvAdapter). Registering write
    # interest here is what schedules the flush; nothing writes to the
    # socket outside on_writable.
    def write(data)
      return nil if @closed

      @buffer << data
      return close if @buffer.bytesize > MAX_BUFFER_BYTES

      @monitor.interests = :rw if @monitor
      nil
    end

    def send_text(text)
      return nil if @closed
      @driver.text(text)
      nil
    rescue StandardError
      close
    end

    def send_json(value)
      send_text(JSON.generate(value))
    end

    def on_readable
      loop do
        data = @socket.read_nonblock(4096, exception: false)
        return nil if data == :wait_readable
        return close if data.nil?

        @driver.parse(data)
        return nil if @closed
      end
    rescue IOError, SystemCallError, EOFError
      close
    end

    # Byte-oriented throughout: appending a UTF-8 frame to the buffer can
    # promote its encoding, so a character-indexed slice would corrupt a
    # partial write of anything non-ASCII.
    def on_writable
      return nil if @closed || @buffer.empty?

      written = @socket.write_nonblock(@buffer, exception: false)
      return nil if written == :wait_writable

      @buffer = if written >= @buffer.bytesize
        +""
      else
        @buffer.byteslice(written, @buffer.bytesize - written)
      end

      @monitor.interests = :r if @buffer.empty? && @monitor
      nil
    rescue IOError, SystemCallError
      close
    end

    def close
      return nil if @closed

      @closed = true
      Registry.unsubscribe_all(self)
      Reactor.remove(self)
      begin
        @socket.close
      rescue IOError, SystemCallError
        nil
      end
      nil
    end

    private

    def handle_message(raw)
      message = JSON.parse(raw)
      return nil unless message["command"] == "subscribe"

      identifier_json = message["identifier"]
      return nil if identifier_json.nil?

      identifier = JSON.parse(identifier_json)
      stream = decode_stream_name(identifier["signed_stream_name"])
      return nil if stream.nil?

      @subscriptions[stream] = identifier_json
      Registry.subscribe(stream, self)
      send_text(%({"identifier":#{JSON.generate(identifier_json)},"type":"confirm_subscription"}))
    rescue JSON::ParserError
      nil
    end

    # Match `turbo_stream_from`'s emit: `<base64-of-JSON>--<sig>`. The
    # placeholder sig today is "unsigned" (see action_view.rb); once real
    # HMAC signing lands the signature gets verified here.
    def decode_stream_name(signed)
      return nil if signed.nil?

      encoded, _sig = signed.split("--", 2)
      return nil if encoded.nil?

      JSON.parse(Base64.strict_decode64(encoded))
    rescue ArgumentError, JSON::ParserError
      nil
    end
  end

  # websocket-driver's `Driver.rack` wants a Rack-env-ish object plus a
  # `write(data)` sink. Headers come from the original env; output goes
  # to the connection's buffer rather than the socket, so the driver
  # never blocks and never writes off the reactor thread.
  class EnvAdapter
    def initialize(env, conn)
      @env = env
      @conn = conn
    end

    attr_reader :env

    def url
      scheme = @env["HTTPS"] == "on" ? "wss" : "ws"
      host = @env["HTTP_HOST"] || "localhost"
      "#{scheme}://#{host}#{@env["PATH_INFO"]}"
    end

    %w[REQUEST_METHOD HTTP_CONNECTION HTTP_UPGRADE HTTP_HOST
       HTTP_ORIGIN HTTP_SEC_WEBSOCKET_KEY HTTP_SEC_WEBSOCKET_VERSION
       HTTP_SEC_WEBSOCKET_PROTOCOL HTTP_SEC_WEBSOCKET_EXTENSIONS].each do |key|
      define_method(key.downcase) { @env[key] }
    end

    def write(data)
      @conn.write(data)
    end
  end

  # Hijack the socket out of Puma and hand it to the reactor. Called
  # from config.ru's Rack lambda on a request thread; returns as soon as
  # the attach is queued.
  def self.upgrade(env)
    socket = env["rack.hijack"].call
    conn = Connection.new(env, socket)
    Reactor.attach(conn)
    conn
  end
end
