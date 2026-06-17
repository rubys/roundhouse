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
  class WsMessage < Tep::WebSocket::Handler
    attr_accessor :ws

    def initialize
      super
      @ws = Tep::WebSocket::Driver.new(0)
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
    drv.set_on_message(cb_msg)

    res.start_websocket(hs.accept_key, drv)
    true
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
