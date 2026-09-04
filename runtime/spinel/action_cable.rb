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
# The bundled `json` (spinel: the native binding to sp_json.c, activated
# by this require; CRuby: the stdlib) parses subscribe frames and quotes
# envelope fields. The flat-key decoder this glue used to carry is gone.
require "json"
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
      Broadcasts.log_append({ action: :message, stream: stream, payload: json })
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

    # `ActionCable.server.pubsub` — the queue an app's OWN test asks
    # what was published. Rails' seam, not ours: campfire's
    # `turbo_test_helper` reads it directly. See the overlay sibling for
    # the argument; this is the same view of the same log.
    def pubsub
      Pubsub.new
    end
  end

  # Rails' test subscription adapter, on `Broadcasts::LOG`.
  #
  # DIVERGES FROM THE OVERLAY IN ONE PLACE, and it is the divergence
  # this file's header already describes: a raw publish's payload was
  # rendered to JSON TEXT at the boundary here (a strict target has no
  # lane for the untyped Hash the overlay carries), so it is handed back
  # as it stands rather than encoded a second time. A turbo entry is
  # rebuilt through `Broadcasts.render_fragment` — the same function
  # `record` used — and encoded, which is what Rails stores.
  class Pubsub
    def broadcasts(stream)
      out = []
      Broadcasts.log.each do |entry|
        next if entry[:stream] != stream
        if entry[:action] == :message
          out << entry[:payload].to_s
        else
          out << JSON.generate(Broadcasts.render_fragment(
            action: entry[:action],
            target: entry[:target],
            html: entry[:html],
            attributes: entry[:attributes].to_s,
          ))
        end
      end
      out
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
      out = out + JSON.generate(key.to_s) + ":" + value.to_s
    end
    out + "}"
  end

  module Channel
    class Base
      # A channel is an ORDINARY OBJECT with no transport in it: it is
      # built against a connection, `subscribed` runs, and the caller
      # reads `streams` and `subscription_rejected?` back off it. The
      # socket, the fd table and the fan-out live in `Cable`, which is
      # what lets a channel's authorization decision be exercised
      # without a server — the same split the CRuby overlay's
      # `Cable::Dispatch` was designed around.
      #
      # These bodies used to raise ("subscriptions are not dispatched
      # yet"), which was true and is the reason a green
      # `presence_channel_test.rb` said nothing about this lane:
      # Rails' `ActionCable::Channel::TestCase` builds the channel
      # itself, so the suite never asked the runtime for one.

      # `identifier` is the identifier JSON STRING exactly as the client
      # sent it — every frame back to that client echoes it byte for
      # byte, because the client keys its subscription table on it. A
      # re-spelled identifier is a frame nobody claims.
      def initialize(connection, identifier, params)
        @connection = connection
        @identifier = identifier
        @params = params
        @streams = []
        @rejected = false
      end

      def connection
        @connection
      end

      def identifier
        @identifier
      end

      def params
        @params
      end

      # THE TWO LIFECYCLE CALLBACKS, as no-ops.
      #
      # Every channel in an ingested tree overrides `subscribed`, so
      # nothing reaches this body — and that is exactly why it has to
      # exist. `Cable.subscribe` holds one channel in a slot and calls
      # `subscribed` on it; on a target that resolves every call
      # statically the method must be declared on the class the slot is
      # typed as, or there is nothing to dispatch through. Rails'
      # `Channel::Base` defines them the same way and for the same
      # reason a base class does anywhere: a channel that only listens
      # is allowed to say nothing.
      #
      # `unsubscribed` is not called on this lane yet — teardown drops
      # fds (`Tep::WebSocket::Connection.dispatch_close` runs
      # `Tep::Broadcast.unsubscribe_fd`) and no channel object survives
      # the frame that built it. Declared with its sibling because the
      # pair is one contract, and a half-declared one invites a caller
      # to assume the other half runs.
      def subscribed
        nil
      end

      def unsubscribed
        nil
      end

      # THE CALLBACK HOOKS, generated by `ingest::channel_callbacks` on a
      # channel that declares any and no-ops here for every channel that
      # does not — the same contract `subscribed` has, and for the same
      # reason: the dispatcher holds one channel in a slot and calls
      # through the class the slot is typed as.
      #
      # Rails spells these as an `ActiveSupport::Callbacks` chain
      # (`on_subscribe :present, unless: :subscription_rejected?`); the
      # pass inlines the chain, guards and all, into one method body,
      # because a chain walked at run time is a list of Symbols a static
      # target cannot dispatch.
      #
      # BOTH RUN. `Cable::WsMessage` holds the channels it confirmed and
      # `Cable::WsClose` walks them on socket close, so the unsubscribe
      # half is not a declaration with nothing behind it — campfire's
      # `on_unsubscribe :absent` takes the `memberships` row back out
      # when the tab goes away, which is the other half of what a
      # connect/disconnect storm costs.
      def after_subscribe
        nil
      end

      def after_unsubscribe
        nil
      end

      # What `subscribed` asked for, in the order it asked. The CALLER
      # registers these; nothing here touches the transport.
      def streams
        @streams
      end

      # The connection identifier campfire's seven channels all read.
      # Spelled out rather than generated from `identified_by` for the
      # reason `Connection::Base` gives below: one name is what
      # `identified_by` has ever been given, and an accessor defined
      # from a computed name is not statically resolvable.
      #
      # nil on a connection that carries no identity. A channel that
      # needs a user then fails on nil, which is the honest outcome — an
      # app whose channels need a user and whose connection does not
      # identify one has a hole, and swallowing it here would hide the
      # hole rather than the error.
      def current_user
        if @connection.nil?
          return nil
        end
        @connection.current_user
      end

      def stream_from(broadcasting)
        @streams << broadcasting.to_s
        nil
      end

      # `broadcasting_for` is spelled ONCE, on the class, because
      # `stream_for` subscribes to that name and `broadcast_to`
      # publishes on it: two spellings would be a subscription nobody
      # ever reaches, and only the browser would notice.
      def stream_for(record)
        stream_from(self.class.broadcasting_for(record))
      end

      # The PUBLISH half of the channel API — `broadcast_to @room,
      # action: :start` in campfire's TypingNotificationsChannel.
      #
      # IMPLEMENTED NOW, and the reason is that `stream_for` above
      # subscribes to the SAME name. It used to raise on the grounds
      # that nothing was listening on `broadcasting_for(record)`, which
      # was true while `stream_for` raised too; leaving the raise in
      # once the subscribe half works would be a false claim about the
      # runtime rather than a recorded gap.
      #
      # Still UNREACHABLE, for a different reason: the only two call
      # sites are `TypingNotificationsChannel#start` and `#stop`, which
      # are inbound channel ACTIONS, and `Cable.handle_message` acts on
      # `subscribe` alone. That is #71 item 6, deliberately outside the
      # MVP — so this is correct-and-unreachable rather than a stub, and
      # nothing has to change here when an action is first routed.
      def broadcast_to(record, message)
        ActionCable.server.broadcast(self.class.broadcasting_for(record), message)
      end

      # A rejection is recorded, not raised. The caller turns it into
      # the `reject_subscription` frame Action Cable's client expects,
      # and a raise here would be indistinguishable from a channel that
      # crashed.
      def reject
        @rejected = true
        nil
      end

      def subscription_rejected?
        @rejected
      end

      # `RoomChannel` -> `"room"`; `Turbo::StreamsChannel` ->
      # `"turbo:streams"`. actioncable 8.0:
      #
      #   name.sub(/Channel$/, "").gsub("::", ":").underscore
      #
      # NO MEMO. The overlay caches this in a class ivar; a class-level
      # `||=` is not a shape this lane should lean on, and the string is
      # short.
      def self.channel_name
        Base.underscore(Base.strip_channel_suffix(name).gsub("::", ":"))
      end

      def self.strip_channel_suffix(text)
        if text.length > 7 && text[text.length - 7, 7] == "Channel"
          return text[0, text.length - 7]
        end
        text
      end

      # actioncable's `serialize_broadcasting` asks the record for
      # `to_gid_param` and falls back to `to_param`; every lowered model
      # is given a `to_gid_param` (see `lower::broadcasts`), so there is
      # nothing to fall back FROM and no `respond_to?` here.
      def self.broadcasting_for(record)
        channel_name + ":" + record.to_gid_param
      end

      # Class names only — `RoomsChannel` shapes, never a path or a word
      # with digits. activesupport's `underscore` also swaps "::" for
      # "/" and strips inflector acronyms; the caller has already
      # replaced "::" and no channel name in the corpus carries an
      # acronym, so this is the CamelCase-to-snake_case half alone.
      def self.underscore(text)
        out = String.new
        i = 0
        while i < text.length
          c = text[i]
          if c >= "A" && c <= "Z"
            if i != 0 && out[out.length - 1] != "_" && out[out.length - 1] != ":"
              out << "_"
            end
            out << c.downcase
          else
            out << c
          end
          i = i + 1
        end
        out
      end
    end

    # A channel's `params` — the subscribe frame's identifier, read one
    # key at a time.
    #
    # PARSED PER READ, COERCED AT THE BOUNDARY. An identifier is
    # `{"channel":"RoomChannel","room_id":5}`: heterogeneous values
    # under keys only the app knows, which is the untyped bag this
    # pipeline has nowhere to put. `JSON.parse` hands it back as one
    # boxed value and every read coerces what it takes out (`to_s`), so
    # nothing downstream widens; a channel only ever asks for the keys
    # it names — campfire's seven ask for `signed_stream_name` and
    # `room_id` — so nothing keeps the object around.
    #
    # A STRING EVERY TIME, INCLUDING FOR A NUMBER. `RoomChannel` does
    # `Room.find_by(id: params[:room_id])` and the client writes
    # `{"room_id":5}` — a JSON number. Answering it as its digits is
    # what makes the find work: the column is an integer and the
    # adapter casts a numeric string, where a Ruby Integer and a Ruby
    # String cannot both come out of one slot on a strict target.
    # Rails' own `params` is string-valued for the same reason one hop
    # earlier (a query string has no types), so this is the Rails
    # answer rather than a concession.
    #
    # "" for a key the identifier does not carry, which is what a
    # channel testing `if params[:signed_stream_name]` needs — a nil
    # would make the slot nullable for every reader.
    class Parameters
      def initialize(identifier)
        @identifier = identifier
      end

      # The text as an object, or nil when it is not one: a frame that
      # does not parse, or parses to an array or a scalar, reads as
      # having no keys rather than raising mid-subscribe. Both runtimes
      # raise JSON::ParserError on bad input; only CRuby would also
      # raise on `[1]["k"]`, so the Hash check does the same job on both.
      def self.object(text)
        begin
          h = JSON.parse(text)
        rescue StandardError
          return nil
        end
        h.is_a?(Hash) ? h : nil
      end

      # One key of `text` as a String: "" when the key is absent or
      # `text` is not an object; a number reads as its digits. The
      # cable glue reads subscribe frames through this too.
      def self.read(text, name)
        h = Parameters.object(text)
        if h.nil?
          return ""
        end
        v = h[name]
        v.nil? ? "" : v.to_s
      end

      def [](key)
        Parameters.read(@identifier, key.to_s)
      end

      def key?(key)
        h = Parameters.object(@identifier)
        h.nil? ? false : h.key?(key.to_s)
      end
    end

  end

  module Connection
    # What `reject_unauthorized_connection` raises. Rails names it the
    # same way, and `Cable.upgrade` is the one place that rescues it: an
    # unauthorized handshake is answered before the socket is taken over,
    # so it can still be given a status.
    module Authorization
      class UnauthorizedError < StandardError
      end
    end

    class Base
      # WHAT `identified_by :current_user` WOULD HAVE WRITTEN.
      #
      # Ingest DROPS that line — it defines an accessor from a computed
      # name, the shape a strict target cannot compile — so the emitted
      # `ApplicationCable::Connection` has no `current_user=` of its own
      # and campfire's `connect` (`self.current_user =
      # find_verified_user`) would be a NoMethodError on the first
      # handshake. One name, because one is what `identified_by` has
      # ever been given.
      attr_accessor :current_user

      # THE COOKIE JAR OF THE HANDSHAKE REQUEST, and the whole reason
      # identity can work at all: a WebSocket upgrade is an ordinary
      # HTTP GET, so it carries the same `Cookie:` header the app's
      # controllers authenticate from. campfire's
      # `ApplicationCable::Connection` includes
      # `Authentication::SessionLookup` and calls
      # `cookies.signed[:session_token]` — the SAME method object its
      # controllers use, reached here through the same shared
      # `ActionController::CookieJar` that `Main.dispatch` builds from
      # `req.cookies`. Handing it a real signed jar is what makes
      # `connect` the app's own code rather than a reimplementation.
      def cookies
        @cookies
      end

      def initialize(cookies)
        @cookies = cookies
      end

      # Rails' Base does not define `connect`; a subclass that wants
      # identity does. A no-op here means the caller can call it
      # unconditionally, so an app whose Connection declares no
      # identifiers connects anonymously instead of erroring.
      def connect
        nil
      end

      # RAISES A CATCHABLE ERROR, where this used to raise
      # NotImplementedError with "connections are not identified yet".
      # That was accurate and is now the wrong shape: a refusal is an
      # ordinary outcome of `connect`, and the caller has to be able to
      # tell it apart from a connection that crashed.
      def reject_unauthorized_connection
        raise Authorization::UnauthorizedError,
              "ActionCable::Connection: the handshake carried no identified user"
      end
    end
  end
end
