# ActionCable — the spinel-subset sibling of the CRuby overlay's
# `ruby_overlay/runtime/action_cable.rb`.
#
# Same two surfaces as the overlay, at opposite ends of the same wire:
#
# 1. `ActionCable.server.broadcast(stream, payload)` — the low-level
#    publish model code calls directly. NOT the Turbo Stream family
#    (`broadcast_append_to` and friends, which lower onto
#    `Broadcasts.append`): those ship an HTML fragment for Turbo to
#    splice into the DOM, this ships an arbitrary payload for the app's
#    own channel JS to read. campfire uses both one line apart — a new
#    message rides Turbo, the unread-room badge beside it rides this.
#
# 2. `Channel::Base` / `Connection::Base` — what an app's own
#    `app/channels/*.rb` subclasses. Channels are ingested as ordinary
#    app classes, so these exist to make those files COMPILE, and to
#    carry the half of a channel that works with no socket at all:
#    `UnreadRoomsChannel.stream_name_for(user_id)` is a pure class
#    method, and the model doing the broadcasting is what calls it.
#
# WHERE THIS DIVERGES FROM THE OVERLAY, and why:
#
# * **The payload is rendered to JSON text here, not carried as a Hash.**
#   The overlay hands a Ruby Hash the whole way to `Cable::Registry
#   .deliver` and lets it serialize. A strict target has no such lane: a
#   `payload` parameter that must hold `{room_id: 1}` and `{roomId: 1}`
#   and whatever the next app writes is exactly the untyped bag the
#   emitter cannot lower. So the Hash is serialized at the boundary —
#   `Server#broadcast` takes it, `payload_json` renders it once, and
#   everything downstream carries String. `Cable.publish_raw` splices
#   that text UNQUOTED into the envelope's `message` field, which is what
#   keeps the shape a JSON object rather than a JSON string.
#
# * **`Broadcasts::LOG` therefore records the rendered text**, where the
#   overlay records the Hash. Both entries answer the question
#   `assert_broadcasts` asks — did a raw publish happen on this stream —
#   and the `action: :message` key still tells the two kinds of entry
#   apart. A reader that inspects `payload` gets text on this target and
#   a Hash on CRuby; that divergence is in docs/pipeline/runtime.md.
#
# SUBSCRIPTION DISPATCH IS NOT IMPLEMENTED, same as the overlay. A client
# naming a channel (`{"channel":"UnreadRoomsChannel"}`) is not routed to
# an instance, so `subscribed` never runs and `stream_from` never
# registers. Turbo's own subscriptions do not come through here at all —
# they arrive carrying a `signed_stream_name` and `Cable.handle_message`
# subscribes them directly, which is why Turbo Stream fan-out works
# without any of this. Until it lands, a raw broadcast reaches every
# connection that subscribed to the same stream name by other means, and
# nobody else.
#
# The instance side therefore RAISES rather than returning quietly: a
# channel that accepts a subscription it will never deliver on is the
# failure that looks like success.
require_relative "broadcasts"
require_relative "cable"

module ActionCable
  # The `ActionCable.server` singleton. Only `broadcast` and
  # `remote_connections` are modeled; `server.config`'s one reachable
  # reader (`mount_path`) is folded to its literal at compile time by
  # `lower::config_reader`, so nothing asks for it here.
  class Server
    # `stream` is the name a subscriber gave (`user_7_unreads`);
    # `payload` is whatever the app wants delivered under `message`.
    #
    # LOG FIRST, then dispatch — the order `Broadcasts.record` uses, so a
    # transport that raises cannot lose the record of the attempt.
    def broadcast(stream, payload)
      json = ActionCable.payload_json(payload)
      Broadcasts::LOG << { action: :message, stream: stream, payload: json }
      Cable.publish_raw(stream, json)
      nil
    end

    # `ActionCable.server.remote_connections.where(current_user: user)
    # .disconnect(reconnect:)` — Rails' "kick this user's sockets" API.
    # campfire calls it from `User#deactivate` and
    # `User#reset_remote_connections`.
    #
    # The set it selects is EMPTY here, and that is a fact about this
    # runtime rather than a stub: a remote connection is identified by
    # its connection identifiers (`current_user`), and no connection in
    # this runtime ever registers one — see the subscription-dispatch
    # note in the header.
    #
    # THE DIVERGENCE THAT COSTS: once subscription dispatch lands, a
    # deactivated or banned user's live socket must actually be closed,
    # and this method is where that happens. Recorded in
    # docs/pipeline/runtime.md rather than left as a silent no-op.
    def remote_connections
      RemoteConnections.new
    end
  end

  # The `where(…)` half of the above: a selection over connections
  # identified by their connection identifiers. Holds no state because
  # the selection is always empty; `disconnect` is what a caller does
  # with it.
  #
  # `current_user:` is spelled out rather than taken as a bag of
  # identifiers — a strict target types a parameter, and this is the one
  # identifier any ingested app has ever named.
  class RemoteConnections
    def where(current_user:)
      RemoteConnection.new
    end
  end

  class RemoteConnection
    def disconnect(reconnect:)
      nil
    end
  end

  SERVER = Server.new

  def self.server
    SERVER
  end

  # Render a raw-publish payload to JSON object text.
  #
  # Integer-valued because that is the whole surface an ingested app has
  # asked for so far (campfire's two call sites are `{room_id: <id>}` and
  # `{roomId: <id>}`). Widening it is a monomorphization decision, not a
  # cast: give the emitter one element type per container and it stays a
  # struct field; make it a bag and every target pays.
  def self.payload_json(payload)
    out = "{"
    first = true
    payload.each do |key, value|
      if !first
        out = out + ","
      end
      first = false
      out = out + Tep::Json.quote(key.to_s) + ":" + value.to_s
    end
    out + "}"
  end

  module Channel
    class Base
      # The four methods the base itself owns. Each is reachable only
      # from a subscription callback, and no subscription callback runs
      # yet — see the header. They raise because the alternative is a
      # `subscribed` that appears to succeed.
      def stream_from(broadcasting)
        raise NotImplementedError,
              "ActionCable::Channel#stream_from: channel subscriptions are not " \
              "dispatched yet (Turbo streams subscribe through Cable directly)"
      end

      def stream_for(record)
        raise NotImplementedError,
              "ActionCable::Channel#stream_for: channel subscriptions are not " \
              "dispatched yet (Turbo streams subscribe through Cable directly)"
      end

      def reject
        raise NotImplementedError,
              "ActionCable::Channel#reject: channel subscriptions are not " \
              "dispatched yet, so there is nothing to reject"
      end

      def params
        raise NotImplementedError,
              "ActionCable::Channel#params: no subscription frame is bound to " \
              "this channel — subscriptions are not dispatched yet"
      end
    end
  end

  module Connection
    class Base
      # `identified_by :current_user` is a CLASS-BODY call that defines
      # an accessor from a computed name — the shape a strict target
      # cannot compile, and ingest drops it from the emitted connection,
      # so the accessor it would have written is declared here instead.
      # One name, because one is what `identified_by` has ever been given.
      attr_accessor :current_user

      def reject_unauthorized_connection
        raise NotImplementedError,
              "ActionCable::Connection#reject_unauthorized_connection: /cable " \
              "connections are not identified yet"
      end
    end
  end
end
