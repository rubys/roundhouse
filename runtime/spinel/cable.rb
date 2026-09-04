# Cable — Action Cable (actioncable-v1-json) WebSocket endpoint for the
# spinel target, built on the Tep::WebSocket codec + Tep::Broadcast fan-out
# under Tep::Server::Threaded.
#
# This is the spinel-subset sibling of the CRuby overlay's
# `ruby_overlay/cable.rb` (which rides Puma's rack-hijack +
# websocket-driver gem). Both satisfy the same surface — Turbo's
# `<turbo-cable-stream-source>` opens a WebSocket to `/cable`, and
# `Broadcasts.set_transport(...)` fans model after-commit fragments out
# to subscribers in real time — but this one uses no gems: the
# connection is a green thread that parks between frames, the
# Tep::WebSocket codec frames the traffic, and Tep::Broadcast does the
# per-connection fan-out.
#
# Protocol implemented (Action Cable v1 JSON):
#   - Server -> client {"type":"welcome"} on open
#   - Server -> client {"type":"ping","message":<unix-ts>} every 3s
#     (ONE heartbeat thread for the process, walking a registry of open
#     connections -- Action Cable's shape, and see `Cable.register` for
#     why it is not a thread per connection; Turbo reconnects without it)
#   - Client -> server {"command":"subscribe","identifier":"<json>"}
#     where the identifier JSON carries
#     {"channel":"Turbo::StreamsChannel","signed_stream_name":"<sig>"}
#   - Server -> client {"identifier":"<json>","type":"confirm_subscription"}
#   - Server -> client {"identifier":"<json>","message":"<turbo-stream>"}
#     when Broadcasts.record fires on a subscribed stream
#
# Single-worker only (WORKERS=1): subscriptions + the broadcast log
# live per-process, so cross-worker fan-out would need the (dropped) PG
# backend. The model after-commit hook and the WebSocket connections
# run in the same worker, so in-process delivery reaches every client.
module Cable
  # The stream-topic -> identifier-JSON map lives on Tep::APP
  # (Tep::APP.cable_identifiers), not a Cable constant: spinel types a
  # Tep.str_hash ivar as StrStrHash but mistypes a module-level
  # constant initialised the same way as int. The identifier is echoed
  # verbatim in confirm_subscription + every broadcast message so Turbo
  # routes the frame to the right stream-source.

  PING_INTERVAL = 3   # seconds — matches Action Cable's default

  # THE APP'S OWN `ApplicationCable::Connection`, built against the
  # handshake's cookie jar — or an anonymous one when the app declares
  # no connection class.
  #
  # GENERATED. `project::apply_cable_connection` rewrites the span
  # between the two markers below from the ingested app, the same way
  # `apply_controller_dispatch` rewrites `Main.instantiate_controller`:
  # the class name arrives as a STRING off the wire nowhere here, but
  # the class itself cannot be reached by `const_get` on a target that
  # resolves every call statically. An eager arm is the whole answer.
  #
  # THE DEFAULT ARM IS NOT A STUB. An app with no
  # `app/channels/application_cable/connection.rb` — the blog fixture —
  # connects ANONYMOUSLY, and must: Turbo Stream fan-out predates
  # identity and has to keep working for an app that never asked for
  # it. `Connection::Base#connect` is a no-op and its `current_user` is
  # nil, so a channel that needs a user fails on nil rather than
  # silently getting somebody else's.
  #
  # NO REQUIRE, deliberately: this is a method BODY reference, and
  # `app/models.rb` (boot.rb, well before any request) has loaded the
  # class by the time a handshake arrives. Requiring it here would
  # invert the load order — `ApplicationCable::Connection`'s superclass
  # is `ActionCable::Connection::Base`, a LOAD-time reference into
  # `runtime/action_cable`, which requires this file back.
  #
  # ONE RETURN TYPE PER TREE, which is why `WsMessage#initialize` builds
  # its placeholder through here too rather than naming the base class
  # directly: two spellings would make the handler's connection slot
  # poly for no gain: a base class is a union point, and a slot with
  # two spellings in it is one the emitter can no longer monomorphize.
  # >>> generated: cable-connection
  def self.build_connection(cookies)
    ActionCable::Connection::Base.new(cookies)
  end
  # <<< generated: cable-connection

  # Resolve the handshake's identity by running the APP's own `connect`.
  #
  # WHY THE APP'S CODE AND NOT A COOKIE LOOKUP HERE: `connect` is where
  # an app decides who is on the other end, and every app decides it
  # differently. campfire's reads `cookies.signed[:session_token]` and
  # loads a `Session`; reimplementing that here would hardcode one app's
  # authentication into the runtime and diverge the moment the app
  # changed it. The runtime's job is to hand `connect` a real signed
  # cookie jar and to honour its verdict — and the jar is the SAME
  # `ActionController::CookieJar` over the SAME `req.cookies` that
  # `Main.dispatch` builds for a controller, because a WebSocket upgrade
  # IS an ordinary HTTP GET and carries the same `Cookie:` header.
  #
  # Returns nil when the app refused. The caller answers that with a
  # status, which is only possible because identity resolves BEFORE
  # `res.start_websocket` — a socket already handed to the recv loop
  # could be closed but not given a reason.
  #
  # No DB lease is taken here: `connect` loads a `Session`, and
  # `Tep::App#dispatch`'s `Db.with_connection` already wraps the whole
  # dispatch, cable branch included.
  def self.identify(req)
    conn = Cable.build_connection(ActionController::CookieJar.new(req.cookies))
    refused = false
    begin
      conn.connect
    rescue ActionCable::Connection::Authorization::UnauthorizedError
      # A refusal is an ORDINARY OUTCOME of `connect`, not a crash, and
      # the rescue is narrowed to exactly the error
      # `reject_unauthorized_connection` raises so that a channel that
      # genuinely broke still reaches the caller as an error.
      refused = true
    end
    if refused
      return nil
    end
    conn
  end

  # THE CHANNEL A SUBSCRIBE FRAME NAMES.
  #
  # GENERATED, between the markers, by `project::apply_cable_channels`
  # — one arm per class in the tree that descends from
  # `ActionCable::Channel::Base`, found by TRANSITIVE descent (campfire's
  # channels are two and three levels deep behind
  # `ApplicationCable::Channel`, so a one-level check finds none of
  # them). The same eager-arm answer `build_connection` above gives, for
  # the same reason: the name arrives as a STRING off the wire and this
  # target has no `const_get`. Only a class the generator wrote an arm
  # for is reachable, so nothing on the wire can widen the set.
  #
  # `Turbo::StreamsChannel` IS ALWAYS AN ARM, and it is not found by
  # descent: it lives in `runtime/turbo_streams.rb`, not in the app.
  # It is also the channel that matters most — a
  # `<turbo-cable-stream-source>` names it unless the page said
  # otherwise, so an app with no channels of its own still needs this
  # one to receive anything at all.
  #
  # nil for a name nothing defined. `subscribe` answers that with
  # `reject_subscription`, which is what Action Cable's client expects
  # and what stops it waiting forever for a confirmation.
  # >>> generated: cable-channels
  def self.build_channel(name, connection, identifier)
    if name == "Turbo::StreamsChannel"
      return Turbo::StreamsChannel.new(
        connection, identifier, ActionCable::Channel::Parameters.new(identifier))
    end
    nil
  end
  # <<< generated: cable-channels

  # Route one subscribe frame to the channel it NAMES, run the app's own
  # `subscribed`, and register whatever streams it asked for.
  #
  # THIS IS WHERE AUTHORIZATION HAPPENS, and the reason item 4 is a
  # dispatch rather than a guard bolted onto the old path: campfire
  # prepends `RoomStreamsAreAuthorized` onto `Turbo::StreamsChannel`,
  # and a prepended module only runs if something calls the method it
  # wraps. The frame used to be decoded here and subscribed directly,
  # so `subscribed` never ran and the guard sat in the tree unreached —
  # whoever could spell a stream name got its fan-out.
  #
  # THE CHANNEL DECIDES THE STREAMS, not this method. `subscribed` calls
  # `stream_from`/`stream_for`, which record on the channel; the loop
  # below registers what it recorded. A channel that authorizes and then
  # subscribes to nothing is confirmed with zero streams, which is what
  # Rails does — the confirmation says "your subscription exists", not
  # "you will receive something".
  #
  # A REJECTION IS A FRAME, not a silence. Action Cable's client keys
  # its subscription table on the identifier and retries a subscription
  # it never heard back about, so a dropped frame is a reconnect loop.
  def self.subscribe(ws, connection, identifier, holder)
    name = ActionCable::Channel::Parameters.read(identifier, "channel")
    if name.length == 0
      return Cable.reject(ws, identifier)
    end
    channel = Cable.build_channel(name, connection, identifier)
    if channel.nil?
      return Cable.reject(ws, identifier)
    end
    # A RAISE IS AN OUTCOME, NOT AN ESCAPE, and on this lane that is
    # load-bearing rather than tidy: tep has no per-request rescue, so
    # an unhandled error inside `subscribed` ends the PROCESS — every
    # other connection with it. `Array#second` reaching the binary
    # un-lowered took the server down on the first cable subscribe.
    # The client is told the subscription was rejected; stderr is the
    # only place the reason can exist, and a subscription that silently
    # stops working is the bug that costs an afternoon. The overlay's
    # `Cable::Dispatch.subscribe` reports the same event the same way.
    begin
      channel.subscribed
      # AFTER `subscribed` AND BEFORE the rejection check, which is
      # Rails' order: the callback's own `unless: :subscription_rejected?`
      # guard is what skips it for a refused subscription, and a channel
      # that declared `on_subscribe` WITHOUT that guard means it to run
      # either way. campfire's `PresenceChannel` writes its `memberships`
      # row here — the whole reason a connect storm costs a database.
      channel.after_subscribe
    rescue StandardError => e
      warn "[cable] " + name + "#subscribed raised: " + e.message
      return Cable.reject(ws, identifier)
    end
    if channel.subscription_rejected?
      return Cable.reject(ws, identifier)
    end
    channel.streams.each do |stream|
      Tep::APP.cable_lock.synchronize do
        Tep::APP.cable_identifiers[stream] = identifier
      end
      Tep::Broadcast.subscribe_ws(stream, ws)
    end
    # REMEMBERED, and only once the subscription is confirmed. The
    # unsubscribe callbacks are the other half of a connect storm —
    # campfire's `on_unsubscribe :absent` is the `memberships` UPDATE
    # that takes a user back out of the room — and they need a channel
    # object that outlives the frame that built it. A rejected or
    # crashed subscribe returns above and is never held.
    holder.remember(channel)
    ws.text("{\"identifier\":" + JSON.generate(identifier) +
            ",\"type\":\"confirm_subscription\"}")
    0
  end

  def self.reject(ws, identifier)
    ws.text("{\"identifier\":" + JSON.generate(identifier) +
            ",\"type\":\"reject_subscription\"}")
    0
  end

  # Handle one inbound WebSocket frame. Only the `subscribe` command is
  # acted on; pings and unsubscribes are ignored (teardown drops fds,
  # and `Broadcast.unsubscribe_fd` runs on close).
  #
  # `connection` is the identified connection this socket was upgraded
  # with — #71 item 3 — and it is passed through to the channel so the
  # app's `subscribed` can read `current_user`. That is the hop item 4
  # adds: identity existed and nothing consulted it.
  def self.handle_message(ws, connection, data, holder)
    frame = ActionCable::Channel::Parameters.object(data)
    if frame.nil?
      return 0
    end
    if frame["command"].to_s != "subscribe"
      return 0
    end
    identifier = frame["identifier"].to_s
    if identifier.length == 0
      return 0
    end
    Cable.subscribe(ws, connection, identifier, holder)
  end

  # ONE HEARTBEAT THREAD FOR THE PROCESS, not one per connection, and
  # the difference is most of our idle cost. A per-connection ping
  # thread sleeps PING_INTERVAL and wakes on its own phase; connections
  # arrive over minutes in a real deployment, so the phases spread out
  # and the scheduler's monitor turns once per beat -- 333 turns a
  # second at 1,000 sockets, each one a poll over every parked thread.
  # A shared beat is one turn per interval no matter how many sockets
  # are held. Measured on the standalone matz and I traded on
  # matz/spinel#4317: 0.375 cores staggered per-connection against
  # 0.010 shared, at 1,000 connections. It is also Action Cable's own
  # shape, which is what the comparison lane runs.
  #
  # THE REGISTRY IS DRIVERS, NOT FDS, for the reason Broadcast's is: an
  # fd number belongs to the next accept the moment its owner closes
  # it, and a heartbeat that wrote to the number would put a ping frame
  # in front of a stranger's 101. `Driver#write_frame` refuses once the
  # connection has retired it, and retirement happens before the close.
  def self.register(ws)
    start = false
    Tep::APP.cable_conns_lock.synchronize do
      Tep::APP.cable_conns.push(ws)
      if Tep::APP.cable_heartbeat == 0
        Tep::APP.cable_heartbeat = 1
        start = true
      end
    end
    # Outside the lock: nothing else should wait on the registry while a
    # thread is being born. The flag is already set, so a second opener
    # racing here cannot start a second beat.
    if start
      Cable.spawn_heartbeat
    end
    0
  end

  # Drop a connection from the heartbeat's registry. Called from the
  # on-close handler, BEFORE the fd is closed, while its number still
  # names only this connection -- the same contract, and the same
  # back-to-front delete, as `Tep::Broadcast.unsubscribe_fd`.
  def self.unregister(ws)
    fd = ws.fd
    dropped = 0
    Tep::APP.cable_conns_lock.synchronize do
      conns = Tep::APP.cable_conns
      i = conns.length - 1
      while i >= 0
        if conns[i].fd == fd
          conns.delete_at(i)
          dropped += 1
        end
        i -= 1
      end
    end
    dropped
  end

  def self.spawn_heartbeat
    Thread.new do
      Cable.heartbeat_loop
    end
    0
  end

  # Beat forever. The thread outlives the last connection and beats over
  # an empty registry rather than exiting: a beat costs one wake every
  # three seconds, and a heartbeat that stops has to be restarted by
  # whoever opens the next socket, under the same lock, which is a race
  # for nothing.
  def self.heartbeat_loop
    while true
      sleep(PING_INTERVAL)
      Cable.beat
    end
    0
  end

  # One beat: the registry copied out under the lock, the frames written
  # outside it -- `Tep::Broadcast.publish_local_only`'s rule, and for the
  # same reason. A slow client can park this thread inside its write,
  # which is the cost of sharing the beat; Action Cable's timer shares
  # the same exposure, and a client that cannot absorb 40 bytes in three
  # seconds is gone anyway. The timestamp is read once per beat rather
  # than once per connection.
  def self.beat
    payload = "{\"type\":\"ping\",\"message\":" + Time.now.to_i.to_s + "}"
    live = [Tep::WebSocket::Driver.new(-1)]
    live.pop
    Tep::APP.cable_conns_lock.synchronize do
      conns = Tep::APP.cable_conns
      i = 0
      while i < conns.length
        live.push(conns[i])
        i += 1
      end
    end
    i = 0
    while i < live.length
      live[i].text(payload)
      i += 1
    end
    live.length
  end

  # on_open handler: greet + join the heartbeat's registry.
  class WsOpen < Tep::WebSocket::Handler
    attr_accessor :ws

    def initialize
      super
      @ws = Tep::WebSocket::Driver.new(0)
    end

    def handle_event(evt)
      @ws.text("{\"type\":\"welcome\"}")
      Cable.register(@ws)
      0
    end
  end

  # on_message handler: subscribe dispatch.
  #
  # THE PER-CONNECTION OBJECT ON THIS LANE. The overlay has one because
  # nio4r hands it a connection; tep hands out an fd and two handler
  # objects — and `Cable.upgrade` builds a FRESH `WsMessage` per
  # upgrade, so the handler instance already IS per-connection. It only
  # lacked identity. Nothing new has to be given a lifecycle: the driver
  # holds the handler (`set_on_message`), so the handler lives exactly
  # as long as the connection and dies with it —
  # `Tep::WebSocket::Connection.dispatch_close` runs `h_close` and then
  # `Tep::Broadcast.unsubscribe_fd(driver.fd)`, and the driver goes.
  class WsMessage < Tep::WebSocket::Handler
    attr_accessor :ws, :connection

    def initialize
      super
      @ws = Tep::WebSocket::Driver.new(0)
      # An anonymous placeholder so the slot has a type before
      # `upgrade` assigns the identified connection. Built through
      # `Cable.build_connection` rather than by naming
      # `ActionCable::Connection::Base` directly so the slot holds ONE
      # class per tree — see that method's note.
      @connection = Cable.build_connection(
        ActionController::CookieJar.new(Tep.str_hash))
      @channels = []
    end

    # UNDER A LEASE. The socket's recv loop runs after the upgrade
    # response, outside the request's `Db.with_connection`, and a
    # subscribe reads the database (`RoomStreamsAreAuthorized` asks who
    # belongs to the room). Without a lease the read fell back to the
    # pool's first connection -- which another thread may hold, and a
    # sqlite connection and its statement cache are one thread's at a
    # time. The first two sockets to subscribe at once took the binary
    # down inside sqlite's parser.
    def handle_event(evt)
      Db.with_connection do
        Cable.handle_message(@ws, @connection, evt.data, self)
      end
      0
    end

    # THE SUBSCRIPTIONS THIS SOCKET HOLDS, so the unsubscribe callbacks
    # have somewhere to run. `Cable.subscribe` pushes a channel here only
    # after it has confirmed it; `WsClose` walks the list once, on the
    # connection's own thread, inside a lease.
    #
    # ONE ARRAY ON THE HANDLER rather than a module-level fd map: the
    # handler already IS the per-connection object (`Cable.upgrade`
    # builds a fresh one per upgrade and the driver holds it), so the
    # list dies with the connection and no lock is needed — nothing else
    # can reach it.
    def remember(channel)
      @channels << channel
      0
    end

    def channels
      @channels
    end

    # Rails runs `unsubscribed` and the `after_unsubscribe` chain when a
    # subscription goes away, including when the socket simply closes.
    #
    # BEST EFFORT, one channel at a time: a teardown callback that raises
    # must not stop the rest of the teardown, and there is nobody left to
    # tell. Same posture as the overlay's `Cable::Dispatch.unsubscribe`.
    def unsubscribe_all
      @channels.each do |channel|
        begin
          channel.unsubscribed
          channel.after_unsubscribe
        rescue StandardError => e
          warn "[cable] unsubscribe raised: " + e.message
        end
      end
      @channels = []
      0
    end
  end

  # on_close handler: run the app's unsubscribe callbacks.
  #
  # A THIRD HANDLER rather than a branch in `WsMessage`, because tep
  # dispatches close to `driver.h_close` and the message handler is not
  # it. It holds the message handler — the object that owns the channel
  # list — which is also what makes the pair one lifetime: `upgrade`
  # builds both and hands the same `WsMessage` to this one.
  #
  # UNDER A LEASE, for the reason `WsMessage#handle_event` is: the
  # callbacks write (`membership.disconnected` is an UPDATE), and this
  # runs on the connection's recv thread outside any request.
  class WsClose < Tep::WebSocket::Handler
    attr_accessor :msg

    def initialize
      super
      @msg = Cable::WsMessage.new
    end

    def handle_event(evt)
      Cable.unregister(@msg.ws)
      Db.with_connection do
        @msg.unsubscribe_all
      end
      0
    end
  end

  # Perform the `/cable` upgrade from inside Main.dispatch. Mirrors the
  # manual shape bin/tep's translator lowers a `websocket` block into:
  # validate the handshake, build one Driver shared by both event
  # handlers, and flip res.start_websocket so Tep::Server::Threaded
  # writes the 101 and runs the recv loop. Returns true if it handled
  # an upgrade (caller returns early), false if the request wasn't a
  # valid WS upgrade.
  def self.upgrade(req, res)
    hs = Tep::WebSocket::Handshake.check(req)
    if !hs.valid
      res.status = 400
      res.body = "invalid websocket upgrade"
      return true
    end

    # IDENTITY RESOLVES BEFORE THE SOCKET IS TAKEN OVER. `res.status`
    # below is only answerable while this is still an ordinary HTTP
    # response; once `res.start_websocket` runs, Tep::Server::Threaded
    # has written the 101 and a refusal could only be a silent close.
    # 401 rather than a quiet welcome, and 401 rather than Rails' 404,
    # because it is what the CRuby overlay's config.ru already answers —
    # the two lanes have to refuse the same way to be comparable.
    conn = Cable.identify(req)
    if conn.nil?
      res.status = 401
      res.body = "unauthorized"
      return true
    end

    drv = Tep::WebSocket::Driver.new(0)
    # Echo the Action Cable subprotocol. The browser's ActionCable client
    # opens with `Sec-WebSocket-Protocol: actioncable-v1-json` and, per its
    # isProtocolSupported() check, IGNORES every frame (welcome,
    # confirm_subscription, and all broadcasts) unless the server echoes a
    # supported subprotocol in the 101 — so without this the
    # `<turbo-cable-stream-source>` never flips to `connected` and no live
    # updates arrive. Select it only when offered (a raw ws client that
    # offers none leaves it "" and the header is omitted, per RFC 6455).
    hs.protocols.each do |proto|
      if proto == "actioncable-v1-json"
        drv.set_subprotocol("actioncable-v1-json")
      end
    end

    cb_open = Cable::WsOpen.new
    cb_open.ws = drv
    cb_open.req = req
    drv.set_on_open(cb_open)

    cb_msg = Cable::WsMessage.new
    cb_msg.ws = drv
    cb_msg.req = req
    cb_msg.connection = conn
    drv.set_on_message(cb_msg)

    cb_close = Cable::WsClose.new
    cb_close.msg = cb_msg
    drv.set_on_close(cb_close)

    res.start_websocket(hs.accept_key, drv)
    true
  end

  # Publish an already-composed Action Cable `message` VALUE to every
  # WebSocket fd subscribed to `stream`.
  #
  # `message_json` is JSON TEXT and is spliced UNQUOTED, which is the
  # whole reason this is separate from `Transport#broadcast` below: that
  # one ships a turbo-stream fragment, an HTML String, and quotes it.
  # Action Cable's `message` field carries the VALUE, so a raw publish of
  # `{roomId: 1}` has to arrive as a JSON OBJECT — quoting it would ship
  # `"message":"{\"roomId\":1}"`, valid JSON in the wrong shape, and
  # wrong in the silent way because only the browser notices.
  #
  # Returns without publishing when no subscriber has named the stream,
  # same as `Transport#broadcast`.
  # The identifier JSON a subscriber sent for `stream`, or "" when none
  # has. Read under the map's lock: subscribes on other connections
  # write it concurrently.
  def self.identifier_for(stream)
    id = ""
    Tep::APP.cable_lock.synchronize do
      id = Tep::APP.cable_identifiers[stream]
    end
    id
  end

  def self.publish_raw(stream, message_json)
    id = Cable.identifier_for(stream)
    if id.length == 0
      return nil
    end
    envelope = "{\"identifier\":" + JSON.generate(id) +
               ",\"message\":" + message_json + "}"
    Tep::Broadcast.publish(stream, envelope)
    nil
  end

  # Broadcasts transport: Broadcasts.record calls broadcast(stream,
  # fragment) on every after-commit hook. Wrap the fragment in the
  # Action Cable message envelope (echoing the subscriber's identifier)
  # and publish to every WS fd subscribed to the stream.
  class Transport
    def broadcast(stream, fragment)
      id = Cable.identifier_for(stream)
      if id.length == 0
        return nil   # no subscriber has named this stream yet
      end
      envelope = "{\"identifier\":" + JSON.generate(id) +
                 ",\"message\":" + JSON.generate(fragment) + "}"
      Tep::Broadcast.publish(stream, envelope)
      nil
    end
  end
end
