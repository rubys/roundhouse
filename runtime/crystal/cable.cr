# Roundhouse Crystal cable runtime.
#
# Action Cable WebSocket + Turbo Streams broadcaster. Mirrors
# runtime/rust/cable.rs + runtime/python/cable.py — same wire
# format (actioncable-v1-json), same partial-renderer registry,
# same per-channel subscriber map.
#
# Uses Crystal's stdlib `HTTP::WebSocket`; no extra shard.

require "base64"
require "http/web_socket"
require "json"

module Roundhouse
  module Cable
    # ── Partial-renderer registry ───────────────────────────────

    @@partial_renderers : Hash(String, Proc(Int64, String)) = {} of String => Proc(Int64, String)

    def self.register_partial(type_name : String, fn : Proc(Int64, String)) : Nil
      @@partial_renderers[type_name] = fn
    end

    def self.render_partial(type_name : String, id : Int64) : String
      fn = @@partial_renderers[type_name]?
      return fn.call(id) if fn
      "<div>#{type_name} ##{id}</div>"
    end

    # ── Turbo Streams rendering ─────────────────────────────────

    def self.turbo_stream_html(action : String, target : String, content : String) : String
      if content.empty?
        %(<turbo-stream action="#{action}" target="#{target}"></turbo-stream>)
      else
        %(<turbo-stream action="#{action}" target="#{target}"><template>#{content}</template></turbo-stream>)
      end
    end

    private def self.dom_id_for(table : String, id : Int64) : String
      singular = table.ends_with?('s') ? table[0, table.size - 1] : table
      "#{singular}_#{id}"
    end

    # ── Broadcast helpers ───────────────────────────────────────

    def self.broadcast_replace_to(table : String, id : Int64, type_name : String, channel : String, target : String) : Nil
      t = target.empty? ? dom_id_for(table, id) : target
      html = render_partial(type_name, id)
      dispatch(channel, turbo_stream_html("replace", t, html))
    end

    def self.broadcast_prepend_to(table : String, id : Int64, type_name : String, channel : String, target : String) : Nil
      t = target.empty? ? table : target
      html = render_partial(type_name, id)
      dispatch(channel, turbo_stream_html("prepend", t, html))
    end

    def self.broadcast_append_to(table : String, id : Int64, type_name : String, channel : String, target : String) : Nil
      t = target.empty? ? table : target
      html = render_partial(type_name, id)
      dispatch(channel, turbo_stream_html("append", t, html))
    end

    def self.broadcast_remove_to(table : String, id : Int64, channel : String, target : String) : Nil
      t = target.empty? ? dom_id_for(table, id) : target
      dispatch(channel, turbo_stream_html("remove", t, ""))
    end

    # ── Subscriber registry + dispatch ──────────────────────────

    # channel name → list of {ws, identifier} pairs. The identifier
    # is the raw subscribe-frame JSON echoed back on every broadcast
    # so Turbo routes the frame to the correct stream-source.
    @@subscribers : Hash(String, Array(Tuple(HTTP::WebSocket, String))) = {} of String => Array(Tuple(HTTP::WebSocket, String))
    @@subscribers_mutex = Mutex.new

    private def self.dispatch(channel : String, html : String) : Nil
      subs = @@subscribers_mutex.synchronize { (@@subscribers[channel]? || ([] of Tuple(HTTP::WebSocket, String))).dup }
      subs.each do |(ws, identifier)|
        msg = {"type" => "message", "identifier" => identifier, "message" => html}.to_json
        begin
          ws.send(msg)
        rescue
          # socket may have closed between our snapshot and now —
          # cleanup happens on the handler side.
        end
      end
    end

    # ── WebSocket handler ───────────────────────────────────────

    # Route /cable — upgrades the connection, runs the
    # actioncable-v1-json flow. Called from server.cr.
    def self.handle(context : HTTP::Server::Context) : Nil
      # Negotiate the subprotocol — Turbo's client requires it.
      wanted = context.request.headers["Sec-WebSocket-Protocol"]? || ""
      if !wanted.includes?("actioncable-v1-json")
        context.response.status_code = 400
        context.response.print "unsupported subprotocol"
        return
      end
      context.response.headers["Sec-WebSocket-Protocol"] = "actioncable-v1-json"

      ws_handler = HTTP::WebSocketHandler.new do |ws, _ctx|
        run_socket(ws)
      end
      ws_handler.call(context)
    end

    private def self.run_socket(ws : HTTP::WebSocket) : Nil
      sub_entries = [] of Tuple(String, Tuple(HTTP::WebSocket, String))

      ws.send({"type" => "welcome"}.to_json)

      # Ping every 3 seconds on a background fiber.
      ping_fiber = spawn do
        loop do
          sleep 3.seconds
          break if ws.closed?
          begin
            ws.send({"type" => "ping", "message" => Time.utc.to_unix}.to_json)
          rescue
            break
          end
        end
      end

      ws.on_message do |msg|
        next unless payload = JSON.parse(msg).as_h?
        next unless payload["command"]? == "subscribe"
        identifier = payload["identifier"]?.try(&.as_s)
        next unless identifier
        channel = decode_channel(identifier)
        next unless channel
        entry = {ws, identifier}
        @@subscribers_mutex.synchronize do
          (@@subscribers[channel] ||= [] of Tuple(HTTP::WebSocket, String)) << entry
        end
        sub_entries << {channel, entry}
        ws.send({"type" => "confirm_subscription", "identifier" => identifier}.to_json)
      rescue
        # malformed JSON etc — silently drop
      end

      ws.on_close do |_code, _reason|
        @@subscribers_mutex.synchronize do
          sub_entries.each do |(channel, entry)|
            if list = @@subscribers[channel]?
              list.delete(entry)
              @@subscribers.delete(channel) if list.empty?
            end
          end
        end
      end
    end

    # Recover the channel name from Turbo's signed_stream_name.
    # Identifier is a JSON object with
    #   {"channel":"Turbo::StreamsChannel",
    #    "signed_stream_name":"<base64>--<digest>"}
    # The base64 prefix decodes to a JSON-encoded channel name.
    private def self.decode_channel(identifier : String) : String?
      id_data = JSON.parse(identifier).as_h?
      return nil unless id_data
      signed = id_data["signed_stream_name"]?.try(&.as_s)
      return nil unless signed
      b64 = signed.split("--", 2).first
      decoded = Base64.decode_string(b64)
      JSON.parse(decoded).as_s
    rescue
      nil
    end

    # Stub kept for compatibility with existing server.cr wiring.
    def self.broadcast(channel : String, body : String) : Nil
      dispatch(channel, body)
    end
  end
end
