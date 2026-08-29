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
# SUBSCRIPTION DISPATCH, and the shape of it. A subscribe frame carries
# an identifier naming a channel (`{"channel":"RoomMessagesChannel",
# "signed_stream_name":"…"}`); `Cable::Dispatch` looks the class up in
# `Channel::Base::REGISTRY`, instantiates it against this connection and
# those params, and runs the app's own `subscribed`. Whatever that method
# asked for through `stream_from`/`stream_for` is what gets registered —
# and if it called `reject`, nothing is.
#
# BY REGISTRY, NOT BY `const_get`. The channel name is a string off the
# wire. Rails resolves it with `safe_constantize` and then checks the
# result descends from `Channel::Base`; here the only names that resolve
# are the ones that registered themselves by being defined, so a crafted
# identifier cannot reach a constant that is not a channel in the first
# place. It is also the rule this runtime already follows: a name
# computed at runtime is not statically resolvable, and eight of the
# targets have no way to honour one.
#
# WHAT THIS BUYS, in one line: the app's authorization runs. campfire
# prepends `RoomStreamsAreAuthorized` onto `Turbo::StreamsChannel` so the
# stock channel refuses `:messages` streams, leaving
# `RoomMessagesChannel` — which checks membership — as the only door onto
# them. Neither ran while subscribes bypassed channels entirely.
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
    # The set it selects is EMPTY here, and that is STILL a divergence
    # after subscription dispatch: connections now carry an identity and
    # channels read `current_user` off it, so the selection is finally
    # expressible — but nothing indexes live connections by user, and
    # `Cable::Reactor`'s table is keyed by socket. Closing it means an
    # index the reactor maintains on attach and drops on close, plus a
    # posted close for each hit.
    #
    # WHAT IT COSTS UNTIL THEN: a deactivated or banned user's open
    # socket keeps its subscriptions. The membership check in
    # `RoomMessagesChannel` runs at SUBSCRIBE time, which is exactly the
    # window campfire's own comment calls out ("revoking a membership
    # disconnects the user with reconnect: true, and the client then
    # replays its subscriptions") — the replay is now authorized, the
    # disconnect that forces it is not. Recorded in
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
    # The base every `app/channels/*.rb` class subclasses, and the half
    # of a subscription that is not the socket: params, identity, and
    # the list of streams a `subscribed` asked for.
    #
    # NOTHING HERE TOUCHES A SOCKET. `Cable::Dispatch` builds one of
    # these on a worker thread, runs `subscribed`, and reads
    # `streams`/`subscription_rejected?` back off it; the reactor thread
    # is what turns that answer into registry entries and a
    # `confirm_subscription` frame. So a channel is an ordinary object
    # with no thread affinity, which is also what makes it testable
    # without a reactor.
    class Base
      # Channel name -> class, populated by `inherited`. THE ONLY WAY a
      # name off the wire resolves: see the file header on why this is a
      # registry and not `const_get`.
      REGISTRY = {}

      # Ruby assigns the constant before calling `inherited`, so `sub.name`
      # is already the real name here. An anonymous subclass (there are
      # none in an ingested tree, but `Class.new(Base)` in a test is one)
      # has a nil name and simply does not register.
      def self.inherited(sub)
        super
        REGISTRY[sub.name] = sub if sub.name
      end

      # nil for a name nothing defined. The caller answers that with
      # `reject_subscription`, which is what Action Cable's client
      # expects for an unknown channel.
      def self.lookup(channel_name)
        REGISTRY[channel_name.to_s]
      end

      # `RoomChannel` -> `"room"`; `Turbo::StreamsChannel` ->
      # `"turbo:streams"`. actioncable 8.0:
      #
      #   @channel_name ||= name.sub(/Channel$/, "").gsub("::", ":").underscore
      def self.channel_name
        @channel_name ||= Base.underscore(name.sub(/Channel\z/, "").gsub("::", ":"))
      end

      # `broadcasting_for([channel_name, record])` — the stream name
      # `stream_for` subscribes to and `broadcast_to` publishes on, so
      # the two must be spelled once. actioncable's `serialize_broadcasting`
      # asks the record for `to_gid_param` and falls back to `to_param`;
      # every lowered model is given a `to_gid_param` (see
      # `lower::broadcasts`), so there is nothing to fall back FROM and no
      # `respond_to?` here.
      def self.broadcasting_for(record)
        channel_name + ":" + record.to_gid_param
      end

      # Class names only — `RoomsController` shapes, never a path or a
      # word with digits. activesupport's `underscore` also swaps "::"
      # for "/" and strips inflector acronyms; the caller above has
      # already replaced "::" and no channel name in the corpus carries
      # an acronym, so this is the CamelCase-to-snake_case half alone.
      def self.underscore(text)
        out = +""
        i = 0
        while i < text.length
          c = text[i]
          if c >= "A" && c <= "Z"
            out << "_" unless i.zero? || out.end_with?("_") || out.end_with?(":")
            out << c.downcase
          else
            out << c
          end
          i += 1
        end
        out
      end

      attr_reader :connection, :identifier, :params, :streams

      # `identifier` is the identifier JSON STRING exactly as the client
      # sent it, because every frame back to that client has to echo it
      # byte for byte — the client keys its subscription table on it.
      def initialize(connection, identifier, params)
        @connection = connection
        @identifier = identifier
        @params = params
        @streams = []
        @rejected = false
      end

      # The connection identifier campfire's channels read. Spelled out
      # rather than generated from `identified_by`, for the reason
      # `runtime/spinel/action_cable.rb` gives one level down: one name
      # is what `identified_by` has ever been given, and a computed
      # accessor is not statically resolvable.
      #
      # nil on an ANONYMOUS connection (an app with no
      # `ApplicationCable::Connection` at all). A channel that reads it
      # will `NoMethodError` on nil — which is the honest outcome: an
      # app whose channels need a user and whose connection class does
      # not identify one has a hole, and swallowing it here would hide
      # the hole rather than the error.
      def current_user
        identity = @connection&.identity
        identity&.current_user
      end

      def stream_from(broadcasting)
        @streams << broadcasting.to_s
        nil
      end

      def stream_for(record)
        stream_from(self.class.broadcasting_for(record))
      end

      # The PUBLISH half. Now that `stream_for` really subscribes,
      # `broadcasting_for(record)` has subscribers and this delivers to
      # them — the reason it used to raise is gone.
      def broadcast_to(record, message)
        ActionCable.server.broadcast(self.class.broadcasting_for(record), message)
      end

      # Refusing is a RECORDED decision rather than a raise: campfire's
      # `PresenceChannel` asks `subscription_rejected?` in an
      # `on_subscribe … unless:` guard, so the answer has to survive the
      # call that produced it.
      def reject
        @rejected = true
        nil
      end

      def subscription_rejected?
        @rejected
      end

      # Rails' Base defines neither; a channel that wants either does.
      # Defining them here means the dispatcher can call both
      # unconditionally.
      def subscribed
        nil
      end

      def unsubscribed
        nil
      end
    end

    # The subscribe frame's identifier, read the way a channel body
    # reads it: `params[:room_id]`, symbol key, against JSON that has
    # only string ones.
    #
    # A two-method value object rather than
    # `ActiveSupport::HashWithIndifferentAccess`: the whole demand is
    # `[]`, and the wide class would arrive with `with_indifferent_access`
    # on every Hash in the tree for one call site.
    class Parameters
      def initialize(raw)
        @raw = raw
      end

      def [](key)
        @raw[key.to_s]
      end

      def key?(key)
        @raw.key?(key.to_s)
      end

      def to_h
        @raw
      end
    end
  end

  module Connection
    # What `reject_unauthorized_connection` raises. Rails names it the
    # same way, and `Cable.upgrade` is the one place that rescues it:
    # an unauthorized handshake is answered 401 and never hijacked, so
    # the socket is closed by Puma rather than parked in the reactor.
    module Authorization
      class UnauthorizedError < StandardError
      end
    end

    class Base
      # `identified_by :current_user` is a CLASS-BODY call, so it runs at
      # LOAD time. A Connection that cannot answer it does not merely
      # fail to identify anybody — the file fails to define.
      def self.identified_by(*names)
        names.each { |name| attr_accessor name }
      end

      # THE COOKIE JAR OF THE HANDSHAKE REQUEST, and the whole reason
      # identity works: a WebSocket upgrade is an ordinary HTTP GET, so
      # it carries the same `Cookie:` header the app's controllers
      # authenticate from. campfire's `ApplicationCable::Connection`
      # includes `Authentication::SessionLookup` and calls
      # `cookies.signed[:session_token]` — the SAME method object its
      # controllers use. Handing it a real signed jar is what makes
      # `connect` the app's code rather than a reimplementation of it.
      attr_reader :cookies

      def initialize(cookies)
        @cookies = cookies
      end

      # Rails' Base does not define `connect`; a subclass that wants
      # identity does. Defining a no-op here means `Cable.upgrade` can
      # call it unconditionally, so an app whose Connection declares no
      # identifiers connects anonymously instead of erroring.
      def connect
        nil
      end

      def reject_unauthorized_connection
        raise Authorization::UnauthorizedError,
              "ActionCable::Connection: the handshake carried no identified user"
      end
    end
  end
end
