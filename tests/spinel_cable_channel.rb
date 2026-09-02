#!/usr/bin/env ruby
# `ActionCable::Channel::Base` on the SPINEL lane, exercised.
#
# Loads `runtime/spinel/action_cable.rb` directly — no emit, no server,
# no socket: `ruby tests/spinel_cable_channel.rb .` reproduces it by
# hand.
#
# WHY THIS FILE EXISTS. Until now every method on this Base raised
# ("channel subscriptions are not dispatched yet"), which was true, and
# which is why the campfire conformance run says nothing about this
# lane: Rails' `ActionCable::Channel::TestCase` builds the channel
# ITSELF and asserts `stream_for` was called, so a green
# `presence_channel_test.rb` is fully compatible with a runtime that
# cannot build a channel at all.
#
# The channels below are campfire's EMITTED forms, copied from the
# transpiled tree rather than from campfire's source — the same rule
# `overlay_cable_dispatch.rb` follows, and for the same reason: the emit
# is what runs. A fixture copied from source is not the tree, which is
# the mistake that cost `identified_by :current_user` a whole debugging
# session on the sibling lane.
#
# THE TRANSPORT IS STUBBED TO LOAD, NOT TO RUN. `action_cable.rb`
# requires `cable.rb`, which subclasses `Tep::WebSocket::Handler` in its
# class bodies, so two empty classes have to exist for the file to
# parse. Nothing below calls them: this Base is a plain object by
# design, and if EXERCISING it ever needs the transport, that is a
# regression in the split rather than a reason to grow the harness.
root = ARGV[0] or abort "usage: spinel_cable_channel.rb <repo-root>"
root = File.expand_path(root)

module Tep
  module WebSocket
    class Handler; end
  end

  # `Tep::APP.cable_identifiers` — read by `Cable.publish_raw`, which
  # nothing here calls.
  class FakeApp
    def cable_identifiers = @cable_identifiers ||= {}
  end
  APP = FakeApp.new
end

load "#{root}/runtime/spinel/action_cable.rb"

FAILURES = []

def check(label, got, want)
  if got == want
    puts "  ok   #{label}"
  else
    FAILURES << label
    puts "  FAIL #{label}: got #{got.inspect}, want #{want.inspect}"
  end
end

puts "vendor runtime/spinel/action_cable.rb"

# --- the connection a channel reads its identity off ------------------
class FakeUser
  def initialize(id) = @id = id
  def id = @id
  def rooms = FakeRooms.new
end

class FakeRooms
  def find_by(id:) = id == 1 ? FakeRoom.new(1) : nil
end

class FakeRoom
  def initialize(id) = @id = id
  def id = @id
  def to_gid_param = "gid://campfire/Room/#{@id}"
end

class FakeConnection
  def initialize(user) = @current_user = user
  def current_user = @current_user
end

# --- campfire's emitted channels, verbatim ----------------------------
module ApplicationCable
  class Channel < ActionCable::Channel::Base
  end
end

class UnreadRoomsChannel < ApplicationCable::Channel
  def self.stream_name_for(user_id)
    "user_#{user_id}_unreads"
  end

  def subscribed
    stream_from self.class.stream_name_for(current_user.id)
  end
end

class RoomChannel < ApplicationCable::Channel
  def subscribed
    if @room = find_room
      stream_for @room
    else
      reject
    end
  end

  def find_room
    current_user.rooms.find_by(id: params[:room_id])
  end
end

class Turbo
  class StreamsChannel < ApplicationCable::Channel
  end
end

# --- the name a subscribe frame carries -------------------------------
check("channel_name strips the suffix", RoomChannel.channel_name, "room")
check("channel_name snake-cases", UnreadRoomsChannel.channel_name, "unread_rooms")
# The namespaced form is what `Turbo::StreamsChannel` broadcasts under,
# and getting the separator wrong is a subscription nobody reaches.
check("channel_name keeps a namespace as ':'", Turbo::StreamsChannel.channel_name, "turbo:streams")

# --- `stream_from`: what the channel ASKED for ------------------------
conn = FakeConnection.new(FakeUser.new(7))
ch = UnreadRoomsChannel.new(conn, '{"channel":"UnreadRoomsChannel"}', {})
ch.subscribed
check("stream_from records the stream", ch.streams, ["user_7_unreads"])
check("a subscribe that streamed is not rejected", ch.subscription_rejected?, false)
# The identifier is echoed byte for byte: the client keys its
# subscription table on it, so a re-spelled one is a frame nobody claims.
check("the identifier is carried verbatim", ch.identifier, '{"channel":"UnreadRoomsChannel"}')

# --- `stream_for`: the name `broadcast_to` must agree with ------------
room_ch = RoomChannel.new(conn, "{}", { room_id: 1 })
room_ch.subscribed
check("stream_for uses broadcasting_for", room_ch.streams, ["room:gid://campfire/Room/1"])
check("broadcasting_for is spelled once",
      RoomChannel.broadcasting_for(FakeRoom.new(1)), "room:gid://campfire/Room/1")

# --- `reject`: recorded, and NOTHING is registered --------------------
denied = RoomChannel.new(conn, "{}", { room_id: 99 })
denied.subscribed
check("a rejected subscribe is marked", denied.subscription_rejected?, true)
# The half that matters. A rejection that still registered a stream
# would be an authorization check whose answer is discarded — which is
# the divergence this lane is ledgered for.
check("a rejected subscribe registers NOTHING", denied.streams, [])

# --- identity ---------------------------------------------------------
check("current_user comes off the connection", ch.current_user.id, 7)
# nil, not a raise: an app with no connection identity connects
# anonymously, and Turbo fan-out predates identity.
anon = UnreadRoomsChannel.new(nil, "{}", {})
check("an anonymous connection has no current_user", anon.current_user, nil)

# --- the connection half: identity off the handshake's cookie jar -----
#
# A WebSocket upgrade is an ordinary HTTP GET, so it carries the same
# `Cookie:` header the controllers authenticate from. What is under test
# is that the runtime hands the app's OWN `connect` a real jar and
# honours its verdict — not a reimplementation of campfire's auth.
class FakeJar
  def initialize(values) = @values = values
  def signed = @values
end

module Authentication
  module SessionLookup
    def find_session_by_cookie
      if token = cookies.signed[:session_token]
        token == "good-token" ? FakeSession.new(FakeUser.new(42)) : nil
      end
    end
  end
end

class FakeSession
  def initialize(user) = @user = user
  def user = @user
end

# campfire's EMITTED ApplicationCable::Connection, verbatim.
module ApplicationCable
  class Connection < ActionCable::Connection::Base
    include Authentication::SessionLookup

    def connect
      self.current_user = find_verified_user
    end

    def find_verified_user
      if verified_session = find_session_by_cookie
        verified_session.user
      else
        reject_unauthorized_connection
      end
    end
  end
end

signed_in = ApplicationCable::Connection.new(FakeJar.new({ session_token: "good-token" }))
signed_in.connect
check("connect identifies the user from the signed jar", signed_in.current_user.id, 42)

# A refusal is a CATCHABLE outcome, not a NotImplementedError: the caller
# has to tell "the app said no" apart from "the app crashed", because the
# first is answered with a status and the second is a bug.
refused = ApplicationCable::Connection.new(FakeJar.new({}))
begin
  refused.connect
  check("an unidentified handshake is refused", "no raise", "UnauthorizedError")
rescue ActionCable::Connection::Authorization::UnauthorizedError
  check("an unidentified handshake is refused", "UnauthorizedError", "UnauthorizedError")
end

# A tampered token reaches the app's own lookup and fails there, not in
# the runtime — the point being that the decision stays the app's.
tampered = ApplicationCable::Connection.new(FakeJar.new({ session_token: "forged" }))
begin
  tampered.connect
  check("a forged token is refused", "no raise", "UnauthorizedError")
rescue ActionCable::Connection::Authorization::UnauthorizedError
  check("a forged token is refused", "UnauthorizedError", "UnauthorizedError")
end

# An app that declares no identifiers connects ANONYMOUSLY: Base#connect
# is a no-op so the caller can call it unconditionally.
class BareConnection < ActionCable::Connection::Base; end
bare = BareConnection.new(FakeJar.new({}))
bare.connect
check("a connection with no identifiers is anonymous", bare.current_user, nil)

puts
if FAILURES.empty?
  puts "ALL OK"
  exit 0
else
  puts "#{FAILURES.length} FAILED: #{FAILURES.join(', ')}"
  exit 1
end
