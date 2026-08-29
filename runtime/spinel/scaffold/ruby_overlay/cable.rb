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
#   * There IS a worker pool, and it is small. A subscribe now runs the
#     app's own `subscribed` — `RoomMessagesChannel` resolves a GlobalID
#     and asks `user.rooms.find_by`, campfire's `PresenceChannel` WRITES
#     — so a subscribe is arbitrary app code holding a database handle,
#     and running it on the reactor thread would let one slow query stall
#     every other connection's frames. Ping and delivery stay on the
#     reactor; only app code moves.
#   * The pool shares `Db`'s connection pool with Puma's threads, so
#     `CABLE_WORKERS` above `RAILS_MAX_THREADS` buys queueing inside
#     `Db.with_connection` rather than concurrency. Against
#     `default_transaction_mode: immediate` SQLite there is one writer
#     whatever either number says.
#   * A connection whose outbound buffer passes MAX_BUFFER_BYTES is
#     closed. Action Cable buffers without a ceiling; here one client
#     that stops reading would otherwise be unbounded memory.
#
# THREADING CONTRACT, in one line: everything below runs on the reactor
# thread except `Reactor.post`, `Reactor.attach`, `Registry.broadcast`
# and the `Workers` pool. That is why SUBS and the connection table
# carry no mutex — they have exactly one writer.
#
# The pool is the one place that breaks the single-thread rule, and it
# is fenced: a worker only ever touches the CHANNEL OBJECT it was handed
# (which nothing else has a reference to yet) and hands its answer back
# through `Reactor.post`. No worker reads SUBS, the connection table, a
# driver or a socket.
#
# Single-worker only. Clustered Puma (workers > 1) needs an inter-worker
# pubsub — Redis in campfire's own deployment — behind the same
# `Broadcasts.set_transport` seam this file plugs into.
require "nio"
require "websocket/driver"
require "json"

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

  # The pool that runs APP code.
  #
  # A subscribe frame ends in the app's own `subscribed`, which queries
  # (and in campfire's `PresenceChannel`, writes) — arbitrary work
  # holding a database handle. Action Cable hands each inbound frame to a
  # worker for exactly this reason, and the reason survives the rest of
  # this file's departures from it: the reactor thread is the one thread
  # every OTHER connection's frames go through.
  #
  # FIXED SIZE, NOT ELASTIC. The bound that matters is `Db`'s connection
  # pool, not this queue: a fifth worker on a three-handle pool parks
  # inside `Db.with_connection` instead of doing work. Sized from the
  # same env var Puma reads, so the two move together, and overridable
  # for a benchmark that wants to see the difference.
  #
  # NO PER-CONNECTION ORDERING, deliberately and like Rails: two
  # subscribe frames from one client may settle in either order. They
  # carry different identifiers and register different streams, so the
  # order is not observable — what WOULD be observable is a subscribe
  # racing its own unsubscribe, which `Connection#channels` serializes by
  # only ever being mutated on the reactor thread.
  #
  # An exception in app code is caught at the boundary: it becomes a
  # rejected subscription, not a dead worker. A pool that shrinks by one
  # every time a channel raises is the failure that takes hours to see.
  module Workers
    SIZE = [(ENV["CABLE_WORKERS"] || ENV["RAILS_MAX_THREADS"] || "3").to_i, 1].max
    START_MUTEX = Mutex.new
    QUEUE = Queue.new

    @threads = nil

    def self.post(&block)
      ensure_started
      QUEUE << block
      nil
    end

    def self.ensure_started
      return if @threads

      START_MUTEX.synchronize do
        return if @threads

        @threads = Array.new(SIZE) do |i|
          t = Thread.new { run }
          t.name = "cable-worker-#{i}" if t.respond_to?(:name=)
          t
        end
      end
      nil
    end

    def self.run
      loop do
        task = QUEUE.pop
        begin
          task.call
        rescue StandardError
          # Already reported at the dispatch boundary; swallowed here so
          # one bad frame cannot cost the pool a thread.
          nil
        end
      end
    end
  end

  # Routing a subscribe frame to a channel, and reading back what it
  # asked for.
  #
  # A PLAIN MODULE OVER PLAIN OBJECTS — no socket, no reactor, no
  # registry. That is what makes the authorization decision testable on
  # its own (`tests/overlay_cable_dispatch.rb` runs campfire's real
  # channels through it with no gems installed), and it is also the
  # thread fence: everything here touches only the channel object the
  # caller just created.
  module Dispatch
    # `channel` is nil for a refusal. `streams` is what `subscribed`
    # asked for, in the order it asked.
    Outcome = Struct.new(:channel, :streams, :error)

    def self.rejected?(outcome)
      outcome.channel.nil?
    end

    # Run `klass#subscribed` for `identifier` on this connection.
    #
    # NOTHING IS REGISTERED HERE. The caller decides what to do with the
    # answer, on the thread that owns the registry.
    #
    # A raise is an outcome, not an escape: campfire's `room_from`
    # rescues `RecordNotFound` itself, but a channel that hits an
    # unrescued error must refuse the subscription rather than leave the
    # client waiting for a confirmation that never comes.
    def self.subscribe(klass, connection, identifier_json, identifier)
      channel = klass.new(
        connection, identifier_json, ActionCable::Channel::Parameters.new(identifier)
      )
      channel.subscribed
      if channel.subscription_rejected?
        Outcome.new(nil, [], nil)
      else
        Outcome.new(channel, channel.streams, nil)
      end
    rescue StandardError => e
      # REPORTED, not just recorded. The client is told "rejected" and
      # cannot be told why, so stderr is the only place the reason
      # exists — and a subscription that silently stops working is the
      # bug that costs an afternoon. Rails logs the same event.
      warn "[cable] #{klass}#subscribed raised: #{e.class}: #{e.message}"
      Outcome.new(nil, [], e)
    end

    # The mirror, for `unsubscribe` and for close. Best-effort: a
    # teardown callback that raises must not stop the rest of the
    # teardown, and there is nobody left to tell.
    def self.unsubscribe(channel)
      channel.unsubscribed
      nil
    rescue StandardError => e
      warn "[cable] #{channel.class}#unsubscribed raised: #{e.class}: #{e.message}"
      nil
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
        # A connection can hold more than one subscription onto the same
        # stream — two channels whose `subscribed` named it, or the same
        # channel with different params — and Action Cable delivers one
        # frame per SUBSCRIPTION, keyed by its identifier. Before channel
        # dispatch a connection had exactly one, so this read a single
        # value; the plural is what dispatch makes possible.
        conn.identifiers_for(stream).each do |identifier|
          frame = frames[identifier] ||=
            %({"identifier":#{JSON.generate(identifier)},"message":#{message_json}})
          conn.send_text(frame)
        end
      end
      nil
    end

    def self.subscribe(stream, conn)
      list = (SUBS[stream] ||= [])
      list << conn unless list.include?(conn)
      nil
    end

    def self.unsubscribe(stream, conn)
      list = SUBS[stream]
      return nil if list.nil?
      list.delete(conn)
      SUBS.delete(stream) if list.empty?
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

    # `identity` is the app's own `ApplicationCable::Connection`,
    # already `connect`ed on the request thread — or nil for an app that
    # declares none. Everything a channel asks about the subscriber
    # (`current_user`) is read off it.
    attr_reader :identity

    def initialize(env, socket, identity = nil)
      @socket = socket
      @buffer = +""
      @subscriptions = {}   # stream name → [identifier JSON], echoed back
      @channels = {}        # identifier JSON → channel, or :pending
      @monitor = nil
      @closed = false
      @identity = identity

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

    def identifiers_for(stream)
      @subscriptions[stream] || []
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
      # The app's own teardown, before the socket goes: campfire's
      # `PresenceChannel#absent` marks a membership disconnected, and a
      # closed browser tab is the ONLY way that method is ever reached.
      # Off the reactor thread because it writes.
      live = @channels.values.reject { |c| c == :pending }
      @channels.clear
      unless live.empty?
        Workers.post { Db.with_connection { live.each { |c| Dispatch.unsubscribe(c) } } }
      end
      Registry.unsubscribe_all(self)
      Reactor.remove(self)
      begin
        @socket.close
      rescue IOError, SystemCallError
        nil
      end
      nil
    end

    # Reactor thread — the worker's answer coming home.
    #
    # RE-CHECKS `@closed`, because everything about this method is late:
    # the client can be gone by the time a query finishes, and
    # registering streams for a dead connection would leave entries in
    # SUBS that `unsubscribe_all` has already run past.
    def subscribe_settled(identifier_json, outcome)
      return nil if @closed
      return nil unless @channels[identifier_json] == :pending

      if Dispatch.rejected?(outcome)
        @channels.delete(identifier_json)
        return send_text(
          %({"identifier":#{JSON.generate(identifier_json)},"type":"reject_subscription"})
        )
      end

      @channels[identifier_json] = outcome.channel
      outcome.streams.each do |stream|
        list = (@subscriptions[stream] ||= [])
        list << identifier_json unless list.include?(identifier_json)
        Registry.subscribe(stream, self)
      end
      send_text(
        %({"identifier":#{JSON.generate(identifier_json)},"type":"confirm_subscription"})
      )
    end

    private

    # THE FRAME THAT DECIDES WHO HEARS WHAT.
    #
    # This used to read `signed_stream_name` straight out of the
    # identifier and subscribe to whatever it decoded to, skipping the
    # channel the client had named. That is precisely the bypass
    # campfire's `RoomStreamsAreAuthorized` exists to close, so the one
    # app in the corpus that had thought about cable authorization had
    # its answer discarded. Now the named channel is what runs, and what
    # it asks for is all that gets registered.
    def handle_message(raw)
      message = JSON.parse(raw)
      identifier_json = message["identifier"]
      return nil if identifier_json.nil?

      case message["command"]
      when "subscribe"   then begin_subscribe(identifier_json)
      when "unsubscribe" then begin_unsubscribe(identifier_json)
      else nil
      end
    rescue JSON::ParserError
      nil
    end

    # Reactor thread: resolve the channel class and hand the app's code
    # to a worker. Everything after this returns is asynchronous.
    def begin_subscribe(identifier_json)
      identifier = JSON.parse(identifier_json)
      return nil unless identifier.is_a?(Hash)

      # An identifier the client already has a subscription (or a
      # pending one) for is a duplicate — Action Cable's client replays
      # its whole table on reconnect, and a reconnect on a socket that
      # never dropped would otherwise run `subscribed` twice.
      return nil if @channels.key?(identifier_json)

      klass = ActionCable::Channel::Base.lookup(identifier["channel"])
      if klass.nil?
        # A name no channel registered. Rails logs and drops; refusing
        # explicitly is better for a client that would otherwise wait
        # forever for a confirmation.
        return send_text(
          %({"identifier":#{JSON.generate(identifier_json)},"type":"reject_subscription"})
        )
      end

      @channels[identifier_json] = :pending
      conn = self
      Workers.post do
        outcome = Db.with_connection do
          Dispatch.subscribe(klass, conn, identifier_json, identifier)
        end
        Reactor.post { conn.subscribe_settled(identifier_json, outcome) }
      end
    end

    # Reactor thread. The registry entries go NOW — they are ours — and
    # only the app's `unsubscribed` callback goes to a worker.
    def begin_unsubscribe(identifier_json)
      channel = @channels.delete(identifier_json)
      return nil if channel.nil? || channel == :pending

      @subscriptions.each_value { |list| list.delete(identifier_json) }
      @subscriptions.each do |stream, list|
        Registry.unsubscribe(stream, self) if list.empty?
      end
      @subscriptions.delete_if { |_stream, list| list.empty? }
      Workers.post { Db.with_connection { Dispatch.unsubscribe(channel) } }
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

  # Resolve the connection's identity from the handshake, by running the
  # APP's own `ApplicationCable::Connection#connect`.
  #
  # WHY THE APP'S CODE AND NOT A COOKIE LOOKUP HERE: `connect` is where
  # an app decides who is on the other end, and every app decides it
  # differently. campfire's reads `cookies.signed[:session_token]` and
  # loads a `Session`; reimplementing that here would hardcode one app's
  # authentication into the runtime and silently diverge the moment the
  # app changed it. The runtime's job is to hand `connect` a real cookie
  # jar and to honour its verdict.
  #
  # Returns the connected instance, or nil when the app rejected the
  # handshake — which `upgrade` turns into a refusal rather than an
  # anonymous socket.
  #
  # An app with no `ApplicationCable::Connection` at all (nothing in
  # `app/channels/`) connects ANONYMOUSLY rather than being refused:
  # Turbo stream fan-out predates identity and must keep working for an
  # app that never asked for it.
  def self.identify(env)
    klass = connection_class
    return NO_IDENTITY if klass.nil?

    jar = ActionController::CookieJar.new(CgiIo.parse_cookies(env["HTTP_COOKIE"]))
    connection = klass.new(jar)
    connection.connect
    connection
  rescue ActionCable::Connection::Authorization::UnauthorizedError
    nil
  end

  # Sentinel for "this app declares no connection class", kept distinct
  # from nil so `upgrade` can tell "no identity wanted" from "identity
  # wanted and refused".
  NO_IDENTITY = :anonymous

  # `ApplicationCable::Connection` is Rails' fixed convention name — the
  # same convention `runtime/action_cable.rb` already encodes by giving
  # `ActionCable::Connection::Base` a body. Guarded because an ingested
  # app need not have channels at all.
  def self.connection_class
    return nil unless defined?(ApplicationCable::Connection)
    ApplicationCable::Connection
  end

  # Hijack the socket out of Puma and hand it to the reactor. Called
  # from config.ru's Rack lambda on a request thread; returns as soon as
  # the attach is queued.
  #
  # NIL when the app refused the handshake. Identity is resolved BEFORE
  # the hijack on purpose: a refusal then still has an intact Rack
  # connection to answer 401 on, where a rejected socket already taken
  # out of Puma could only be closed without a status.
  def self.upgrade(env)
    identity = identify(env)
    return nil if identity.nil?

    socket = env["rack.hijack"].call
    conn = Connection.new(env, socket, identity == NO_IDENTITY ? nil : identity)
    Reactor.attach(conn)
    conn
  end
end
