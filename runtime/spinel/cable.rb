# Cable — Action Cable (actioncable-v1-json) WebSocket endpoint for the
# spinel target, built on tep's WebSocket + Scheduler + Broadcast stack.
#
# This is the spinel-subset sibling of the CRuby overlay's
# `ruby_overlay/cable.rb` (which rides Puma's rack-hijack +
# websocket-driver gem). Both satisfy the same surface — Turbo's
# `<turbo-cable-stream-source>` opens a WebSocket to `/cable`, and
# `Broadcasts.set_transport(...)` fans model after-commit fragments out
# to subscribers in real time — but this one uses no threads, no gems:
# tep's fiber-scheduled server holds the connection open, tep's
# WebSocket codec frames the traffic, and Tep::Broadcast does the
# per-fd fan-out.
#
# Protocol implemented (Action Cable v1 JSON):
#   - Server -> client {"type":"welcome"} on open
#   - Server -> client {"type":"ping","message":<unix-ts>} every 3s
#     (a per-connection scheduler fiber; Turbo reconnects without it)
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
  # main.rb's `Db.with_connection { Main.dispatch(req, res) }` already
  # wraps the whole dispatch, cable branch included.
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

  # Recover the stream name from Turbo's signed_stream_name:
  # `<base64(JSON(stream))>--<sig>`. Strip the `--` suffix, base64-
  # decode (-> a JSON string like `"articles"`), drop the surrounding
  # quotes. Returns "" on anything malformed.
  def self.decode_stream(signed)
    cut = Tep.str_find(signed, "--", 0)
    b64 = cut < 0 ? signed : signed[0, cut]
    if b64.length == 0
      return ""
    end
    decoded = Base64.strict_decode64(b64)
    # decoded is the JSON-encoded stream name, e.g. "\"articles\"".
    if decoded.length >= 2 && decoded[0] == "\"" && decoded[decoded.length - 1] == "\""
      return decoded[1, decoded.length - 2]
    end
    decoded
  end

  # Handle one inbound WebSocket frame. Only the `subscribe` command is
  # acted on; pings/unsubscribes are ignored (teardown drops fds).
  #
  # NOT AUTHORIZED, and that is a ledgered divergence — see
  # docs/pipeline/runtime.md, "A cable subscribe is not authorized on
  # the SPINEL lane". The frame names a channel and nothing routes on
  # it, so the app's `subscribed` never runs: whoever can spell the
  # stream name gets its fan-out.
  #
  # THE `current_user` NOW EXISTS AND IS STILL NOT CONSULTED — #71 item
  # 3 landed, item 4 did not. `Cable.upgrade` runs the app's `connect`
  # and hands the identified connection to `WsMessage#connection`, so
  # the thing a guard would test against is one hop away; what is
  # missing is the hop. Nothing here reads it yet, and that is the gap
  # rather than an oversight. Closing it is item 4 — routing this frame
  # to the channel it NAMES — not stream-name signing.
  def self.handle_message(ws, data)
    cmd = Tep::Json.get_str(data, "command")
    if cmd != "subscribe"
      return 0
    end
    identifier = Tep::Json.get_str(data, "identifier")
    if identifier.length == 0
      return 0
    end
    signed = Tep::Json.get_str(identifier, "signed_stream_name")
    if signed.length == 0
      return 0
    end
    stream = Cable.decode_stream(signed)
    if stream.length == 0
      return 0
    end
    Tep::APP.cable_identifiers[stream] = identifier
    Tep::Broadcast.subscribe_ws(stream, ws.fd)
    ws.text("{\"identifier\":" + Tep::Json.quote(identifier) +
            ",\"type\":\"confirm_subscription\"}")
    0
  end

  # Spawn a per-connection ping fiber on the cooperative scheduler.
  # Loops every PING_INTERVAL seconds emitting a ping frame; exits when
  # a write fails (the fd closed). One fiber per connection.
  def self.spawn_ping(ws)
    Tep::Scheduler.spawn_fiber(Fiber.new { Cable.ping_loop(ws) })
    0
  end

  def self.ping_loop(ws)
    while true
      Tep::Scheduler.pause(PING_INTERVAL)
      r = ws.text("{\"type\":\"ping\",\"message\":" + Time.now.to_i.to_s + "}")
      if r < 0
        return 0
      end
    end
    0
  end

  # on_open handler: greet + start the ping fiber.
  class WsOpen < Tep::WebSocket::Handler
    attr_accessor :ws

    def initialize
      super
      @ws = Tep::WebSocket::Driver.new(0)
    end

    def handle_event(evt)
      @ws.text("{\"type\":\"welcome\"}")
      Cable.spawn_ping(@ws)
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
    end

    def handle_event(evt)
      Cable.handle_message(@ws, evt.data)
      0
    end
  end

  # Perform the `/cable` upgrade from inside Main.dispatch. Mirrors the
  # manual shape bin/tep's translator lowers a `websocket` block into:
  # validate the handshake, build one Driver shared by both event
  # handlers, and flip res.start_websocket so Tep::Server::Scheduled
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
    # response; once `res.start_websocket` runs, Tep::Server::Scheduled
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
  def self.publish_raw(stream, message_json)
    id = Tep::APP.cable_identifiers[stream]
    if id.length == 0
      return nil
    end
    envelope = "{\"identifier\":" + Tep::Json.quote(id) +
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
      id = Tep::APP.cable_identifiers[stream]
      if id.length == 0
        return nil   # no subscriber has named this stream yet
      end
      envelope = "{\"identifier\":" + Tep::Json.quote(id) +
                 ",\"message\":" + Tep::Json.quote(fragment) + "}"
      Tep::Broadcast.publish(stream, envelope)
      nil
    end
  end
end
