# ActionCable — the framework surface an app's OWN channels sit on, plus
# the low-level publish API that model code calls directly.
#
# Two surfaces, at opposite ends of the same wire.
#
# 1. `ActionCable.server.broadcast(stream, payload)`. This is NOT the
#    Turbo Stream family (`broadcast_append_to` and friends, which lower
#    onto `Broadcasts.append`): those ship an HTML fragment for Turbo to
#    splice into the DOM, this ships an arbitrary payload for the app's
#    own channel JS to read. campfire uses both, one line apart — a new
#    message rides Turbo, the unread-room badge beside it rides this.
#
#    The payload stays a Ruby Hash the whole way to
#    `Cable::Registry.deliver`, and that is what makes the envelope right.
#    Action Cable's `message` field carries the VALUE, so `{roomId: 1}`
#    has to arrive as a JSON object; pre-serializing it to a String here
#    would ship `"message":"{\"roomId\":1}"` — valid JSON, wrong shape,
#    and wrong in the silent way, because `JSON.generate` accepts either
#    happily and only the browser notices.
#
# 2. `Channel::Base` / `Connection::Base` — what `app/channels/*.rb`
#    subclasses. Channels are ingested as ordinary app classes, so these
#    exist to make those files LOAD, and to carry the half of a channel
#    that works with no socket at all: `UnreadRoomsChannel
#    .stream_name_for(user_id)` is a pure class method, and the model
#    doing the broadcasting is what calls it.
#
# SUBSCRIPTION DISPATCH IS NOT IMPLEMENTED. A client naming a channel
# (`{"channel":"UnreadRoomsChannel"}`) is not routed to an instance, so
# `subscribed` never runs and `stream_from` never registers. Turbo's own
# subscriptions do not come through here at all — they arrive carrying a
# `signed_stream_name` and `Cable::Connection#handle_message` subscribes
# them directly, which is why Turbo Stream fan-out works without any of
# this. Until that lands, a raw broadcast reaches every connection that
# subscribed to the same stream name by other means, and nobody else.
#
# The instance side therefore RAISES rather than returning quietly: a
# channel that accepts a subscription it will never deliver on is the
# failure that looks like success.
#
# CRuby/JRuby only, like the `cable.rb` it publishes through. Spinel's
# Action Cable rides tep and is a separate substrate.
require_relative "broadcasts"

module ActionCable
  # The `ActionCable.server` singleton. Only `broadcast` is modeled;
  # `server.config`'s one reachable reader (`mount_path`) is folded to
  # its literal at compile time by `lower::config_reader`, so nothing
  # asks for it here.
  class Server
    # `stream` is the name a subscriber gave (`user_7_unreads`);
    # `payload` is whatever the app wants delivered under `message`.
    #
    # Reaching `Broadcasts::TRANSPORTS` directly rather than adding a
    # `Broadcasts.publish` to broadcasts.rb is deliberate: that file is
    # SHARED with spinel, which compiles it whole. A `payload` parameter
    # with no spinel caller types as int and then poisons the
    # `broadcast(String, String)` dispatch below it — the same trap
    # `SeedTransport` exists to document. This file is overlay-only, so
    # it can hand a Hash across that seam.
    #
    # RECORDED IN `Broadcasts::LOG`, and this file used to carry the
    # opposite conclusion: that LOG's `action`/`target`/`html` shape is
    # the turbo-FRAGMENT contract and a raw payload is not a fragment.
    # True about the shape, wrong about the log. `ActionCable::TestHelper
    # #assert_broadcasts` reads LOG and exists precisely to count raw
    # publishes on an app-named stream — Rails' own reads the test
    # adapter's pubsub queue, which carries whatever was published. With
    # only the dispatch here, campfire's "creating a message broadcasts
    # unread room to each member" counted 0 against a broadcast that had
    # in fact happened: the two halves of the harness were never joined.
    #
    # Logged under `payload:` rather than squeezed into `html:`, so an
    # entry never claims to be markup it is not. `action: :message` is
    # what tells the two kinds of entry apart; both readers of the log
    # (`capture_broadcasts_on`, `capture_turbo_stream_broadcasts`) filter
    # on `:stream` alone, so the extra key costs them nothing.
    #
    # LOG FIRST, then dispatch — the order `Broadcasts.record` uses, so a
    # transport that raises cannot lose the record of the attempt.
    def broadcast(stream, payload)
      Broadcasts::LOG << { action: :message, stream: stream, payload: payload }
      Broadcasts::TRANSPORTS[0].broadcast(stream, payload)
      nil
    end

    # `ActionCable.server.remote_connections.where(current_user: user)
    # .disconnect(reconnect:)` — Rails' "kick this user's sockets"
    # API. campfire calls it from `User#deactivate` and
    # `User#reset_remote_connections`.
    #
    # The set it selects is EMPTY here, and that is a fact about this
    # runtime rather than a stub: a remote connection is identified by
    # its connection identifiers (`current_user`), and no connection in
    # this runtime ever registers one. Turbo's streams subscribe by
    # signed stream name through `Cable::Connection#handle_message`,
    # and channel subscription dispatch — the half that would run
    # `identified_by :current_user` — is not implemented (see the
    # header, and the `Channel::Base` methods below that raise for the
    # same reason). Disconnecting an empty set is a no-op, so this
    # returns without raising instead of pretending to disconnect
    # somebody.
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
  class RemoteConnections
    def where(_identifiers)
      RemoteConnection.new
    end
  end

  class RemoteConnection
    def disconnect(reconnect: false)
      nil
    end
  end

  SERVER = Server.new

  def self.server
    SERVER
  end

  module Channel
    class Base
      # The four methods the base itself owns. Each is reachable only
      # from a subscription callback, and no subscription callback runs
      # yet — see the header. They raise because the alternative is a
      # `subscribed` that appears to succeed.
      def stream_from(_broadcasting)
        raise NotImplementedError,
              "ActionCable::Channel#stream_from: channel subscriptions are not " \
              "dispatched yet (Turbo streams subscribe through Cable directly)"
      end

      def stream_for(_record)
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
      # `identified_by :current_user` is a CLASS-BODY call, so it runs at
      # LOAD time. A Connection that cannot answer it does not merely
      # fail to identify anybody — the file fails to define.
      def self.identified_by(*names)
        names.each { |name| attr_accessor name }
      end

      def reject_unauthorized_connection
        raise NotImplementedError,
              "ActionCable::Connection#reject_unauthorized_connection: /cable " \
              "connections are not identified yet"
      end
    end
  end
end
