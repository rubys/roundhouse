# Cable — minimal Action-Cable-shape WebSocket endpoint for the
# CRuby target. Lives in `ruby_overlay/` because Puma's rack-hijack +
# websocket-driver-gem combo is CRuby-specific; the spinel target
# will land an sphttp-side equivalent that satisfies the same surface
# (`Broadcasts.set_transport(Cable::Registry)`).
#
# Protocol implemented (Action Cable v1 JSON):
#   - Subprotocol negotiation: server echoes "actioncable-v1-json" in
#     the Sec-WebSocket-Protocol response header. Turbo's JS client
#     drops the connection without this.
#   - Server → client `{type: "welcome"}` on connect
#   - Client → server `{command: "subscribe", identifier: '<json>'}`
#     where the identifier JSON carries
#     `{channel: "Turbo::StreamsChannel", signed_stream_name: "<sig>"}`
#   - Server → client `{type: "confirm_subscription", identifier: ...}`
#   - Server → client `{identifier: ..., message: "<turbo-stream>..."}`
#     when `Broadcasts.record` fires on a subscribed stream
#   - Server → client `{type: "ping", message: <unix-ts>}` every 3s
#     (per-connection thread). Required — the JS client times out and
#     reconnects without periodic pings.
#
# Single-worker only. Clustered Puma (workers > 1) would need an
# inter-worker pubsub (Redis equivalent); deferred until measurement
# motivates it.
require "websocket/driver"
require "json"
require "base64"

module Cable
  # In-process registry: stream name → list of connections subscribed
  # to it. Mutex-guarded; connections own their write-side mutex
  # (driver.text isn't thread-safe under contention).
  module Registry
    SUBS = {}
    MUTEX = Mutex.new

    def self.subscribe(stream, conn)
      MUTEX.synchronize do
        list = (SUBS[stream] ||= [])
        list << conn unless list.include?(conn)
      end
    end

    def self.unsubscribe_all(conn)
      MUTEX.synchronize do
        SUBS.each_value { |list| list.delete(conn) }
      end
    end

    def self.broadcast(stream, fragment_html)
      conns = MUTEX.synchronize { (SUBS[stream] || []).dup }
      conns.each { |c| c.push(stream, fragment_html) }
    end
  end

  # Per-connection state. `socket` is the raw TCP IO returned by
  # `env["rack.hijack"].call`. websocket-driver handles the HTTP
  # upgrade handshake (it reads the request line + headers off the
  # socket via the env adapter below) and frame parsing thereafter.
  # Action Cable's browser client requires:
  # 1. Server echoes one of the offered subprotocols ("actioncable-
  #    v1-json") in the Sec-WebSocket-Protocol response header; without
  #    it the JS client refuses the connection.
  # 2. Periodic server-sent `{type: "ping", message: <unix-ts>}` frames.
  #    The client treats absence of pings (default ~6s) as a dead
  #    connection and reconnects in a loop.
  PROTOCOLS = ["actioncable-v1-json"].freeze
  PING_INTERVAL = 3 # seconds — matches Rails' Action Cable default

  class Connection
    def initialize(env, socket)
      @socket = socket
      @write_mutex = Mutex.new
      @subscriptions = {}  # stream_name → identifier_json (for echoing in messages)
      @ping_thread = nil
      @closed = false

      @driver = WebSocket::Driver.rack(EnvAdapter.new(env, socket), protocols: PROTOCOLS)
      @driver.on(:open)    { on_open }
      @driver.on(:message) { |evt| handle_message(evt.data) }
      @driver.on(:close)   { teardown }
      @driver.start
    end

    def run
      # IO loop: read bytes off the socket, feed to driver, repeat
      # until the peer closes. driver.parse fires the registered
      # callbacks (:message, :close) inline on this thread.
      loop do
        data = @socket.readpartial(4096)
        @driver.parse(data)
      end
    rescue EOFError, Errno::ECONNRESET, IOError
      # peer closed
    ensure
      teardown
    end

    # Called by Registry.broadcast. Writes are serialized per-
    # connection because websocket-driver's frame writer isn't
    # safe to call concurrently.
    def push(stream, fragment_html)
      identifier = @subscriptions[stream]
      return if identifier.nil?
      payload = JSON.generate(identifier: identifier, message: fragment_html)
      @write_mutex.synchronize { @driver.text(payload) }
    rescue
      # broken pipe / closed driver → just drop
    end

    private

    def on_open
      send_welcome
      start_ping_thread
    end

    def send_welcome
      @write_mutex.synchronize { @driver.text(JSON.generate(type: "welcome")) }
    end

    # Spawn a background thread that emits ping frames every 3 seconds
    # until the connection closes. Ping payload is `{type: "ping",
    # message: <unix-ts>}` matching Action Cable's wire format —
    # Turbo's JS client checks the timestamp to detect liveness.
    def start_ping_thread
      @ping_thread = Thread.new do
        until @closed
          sleep PING_INTERVAL
          break if @closed
          begin
            payload = JSON.generate(type: "ping", message: Time.now.to_i)
            @write_mutex.synchronize { @driver.text(payload) }
          rescue
            break
          end
        end
      end
    end

    def handle_message(raw)
      msg = JSON.parse(raw)
      cmd = msg["command"]
      identifier_json = msg["identifier"]
      return if identifier_json.nil?

      case cmd
      when "subscribe"
        identifier = JSON.parse(identifier_json)
        stream = decode_stream_name(identifier["signed_stream_name"])
        return if stream.nil?
        @subscriptions[stream] = identifier_json
        Registry.subscribe(stream, self)
        @write_mutex.synchronize do
          @driver.text(JSON.generate(identifier: identifier_json, type: "confirm_subscription"))
        end
      when "unsubscribe"
        # Best-effort: leave per-stream cleanup to teardown.
      end
    rescue JSON::ParserError
      # malformed frame, ignore
    end

    # Match `turbo_stream_from`'s emit:
    #   `<base64-of-JSON-encoded-stream-name>--<sig>`
    # The placeholder sig today is "unsigned" (see action_view.rb's
    # turbo_stream_from). Strip the suffix, base64-decode, JSON-parse.
    # Once real HMAC signing lands the signature gets verified here.
    def decode_stream_name(signed)
      return nil if signed.nil?
      encoded, _sig = signed.split("--", 2)
      return nil if encoded.nil?
      JSON.parse(Base64.strict_decode64(encoded))
    rescue ArgumentError, JSON::ParserError
      nil
    end

    def teardown
      @closed = true
      Registry.unsubscribe_all(self)
      begin
        @socket.close
      rescue IOError
        # already closed
      end
    end
  end

  # websocket-driver's `Driver.rack(env)` expects an object that
  # quacks like a Rack environment plus a `write(data)` method for
  # frame output. We hand it the original env hash for header access
  # and proxy `write` to the hijacked socket.
  class EnvAdapter
    def initialize(env, socket)
      @env = env
      @socket = socket
    end

    def env
      @env
    end

    def url
      scheme = @env["HTTPS"] == "on" ? "wss" : "ws"
      host = @env["HTTP_HOST"] || "localhost"
      "#{scheme}://#{host}#{@env["PATH_INFO"]}"
    end

    %w[REQUEST_METHOD HTTP_CONNECTION HTTP_UPGRADE HTTP_HOST
       HTTP_ORIGIN HTTP_SEC_WEBSOCKET_KEY HTTP_SEC_WEBSOCKET_VERSION
       HTTP_SEC_WEBSOCKET_PROTOCOL HTTP_SEC_WEBSOCKET_EXTENSIONS].each do |k|
      define_method(k.downcase) { @env[k] }
    end

    def write(data)
      @socket.write(data)
    rescue IOError, Errno::EPIPE
      # socket closed mid-write; driver will get a :close event on
      # the next read failure
    end
  end
end
