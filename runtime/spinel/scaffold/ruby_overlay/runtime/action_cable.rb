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
#    `Cable::Connection#push`, and that is what makes the envelope right.
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
    # Not recorded in `Broadcasts::LOG`: LOG's shape
    # (`action`/`target`/`html`) is the turbo-FRAGMENT contract, and a
    # raw payload is not a fragment. A test-visible record for raw
    # publishes belongs with the subscription half, which is what would
    # give it something to assert against.
    def broadcast(stream, payload)
      Broadcasts::TRANSPORTS[0].broadcast(stream, payload)
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
