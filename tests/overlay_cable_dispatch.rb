#!/usr/bin/env ruby
# Named-channel subscribe dispatch on the CRuby overlay, exercised.
#
# Loads the overlay files directly — no emit, no server, no socket:
# `ruby tests/overlay_cable_dispatch.rb .` reproduces it by hand.
#
# WHAT IS UNDER TEST is the authorization decision, not the plumbing.
# `Cable::Dispatch.subscribe` resolves the channel a client NAMED,
# instantiates it, and runs the app's own `subscribed`; the reactor and
# the worker pool exist only to decide which thread that happens on.
# Keeping the decision in a plain module is what lets this file drive it
# with neither nio4r nor websocket-driver installed — the same
# constraint `overlay_cable_identity.rb` documents, and the same
# stubbing.
#
# The channels below are campfire's EMITTED forms, copied from
# `app/models/room_messages_channel.rb` in the transpiled tree rather
# than from campfire's source. That is deliberate and it is the point of
# the test: the emit is what runs, and its `ActiveSupport.present?` /
# `self.class.subscribable_room` spellings are what the runtime has to
# satisfy.
root = ARGV[0] or abort "usage: overlay_cable_dispatch.rb <repo-root>"
root = File.expand_path(root)

require "json"

# --- what boot.rb has standing before cable.rb loads -----------------
load "#{root}/runtime/spinel/base64.rb"
load "#{root}/runtime/ruby/active_support_ext.rb"
load "#{root}/runtime/spinel/scaffold/ruby_overlay/runtime/active_support_core_ext.rb"

# The REAL `GlobalID.param` rides in here, and it has to: this file
# round-trips the mint through `GlobalID::Locator`, and a local copy of
# the encoder would agree with the decoder while both disagreed with the
# page. That is the failure the `turbo_stream_from` commit ran into from
# the other direction — three spellings, two of them in step.
load "#{root}/runtime/ruby/rails.rb"

# `Rails.application` in the shipped runtime is an app-specific reopen
# (`apply_application_reopen` synthesizes `global_id_app` from the module
# wrapping `class Application < Rails::Application`). campfire's is
# "campfire", which is half of every gid below.
module Rails
  def self.application = @application ||= Struct.new(:global_id_app).new("campfire")
end

# action_cable.rb's one require; the transport half is not under test.
$LOADED_FEATURES << File.expand_path(
  "#{root}/runtime/spinel/scaffold/ruby_overlay/runtime/broadcasts.rb"
)
module Broadcasts
  LOG = []
  TRANSPORTS = []
end

load "#{root}/runtime/spinel/scaffold/ruby_overlay/runtime/action_cable.rb"

# turbo_streams.rb requires `action_cable`, `base64` and `broadcasts` as
# SIBLINGS, which
# in the emitted tree are the overlay's (ruby_overlay supersedes the
# spinel file at the same path). In this repo they are the spinel ones,
# so both are marked loaded and the overlay pair above stands instead.
# Loading the spinel action_cable here would replace `Channel::Base` with
# the sibling whose methods raise — a green run against the wrong file.
%w[action_cable base64 broadcasts].each do |sibling|
  $LOADED_FEATURES << File.expand_path("#{root}/runtime/spinel/#{sibling}.rb")
end
load "#{root}/runtime/spinel/turbo_streams.rb"
load "#{root}/runtime/spinel/global_id_locator.rb"

# The reactor's two gems, marked loaded and left EMPTY — see
# overlay_cable_identity.rb for why they are not stubbed. Dispatch never
# reaches a driver or a selector; anything that does dies with a
# NameError naming the constant it wanted.
%w[nio websocket/driver].each { |feature| $LOADED_FEATURES << "#{feature}.rb" }
module NIO; end
module WebSocket; end

load "#{root}/runtime/spinel/scaffold/ruby_overlay/cable.rb"

puts "vendor ruby #{RUBY_VERSION}"

# --- the app half ----------------------------------------------------
# A Room the locator can find, and a User whose `rooms` is a membership
# list rather than every room — the distinction the guard turns on.
class Room
  ALL = {}
  attr_reader :id
  def initialize(id) = @id = id
  def self.find(id)
    ALL[id] or raise ActiveRecord::RecordNotFound, "Couldn't find Room with id=#{id}"
  end
  def to_gid_param = GlobalID.param("Room", id)
end

module ActiveRecord
  class RecordNotFound < StandardError; end
end

class RoomScope
  def initialize(ids) = @ids = ids
  def find_by(id:) = @ids.include?(id) ? Room::ALL[id] : nil
end

class User
  attr_reader :id, :rooms
  def initialize(id, room_ids)
    @id = id
    @rooms = RoomScope.new(room_ids)
  end
end

module ApplicationCable
  class Channel < ActionCable::Channel::Base
  end
end

# campfire's `app/models/room_messages_channel.rb`, as emitted.
class RoomMessagesChannel < ApplicationCable::Channel
  include Turbo::Streams::StreamName::ClassMethods

  STREAM_SUFFIX = "messages"

  def self.guarded_stream?(stream_name)
    stream_name.to_s.split(":", 2).second == STREAM_SUFFIX
  end

  def self.subscribable_room(user, stream_name)
    gid_param, suffix = stream_name.to_s.split(":", 2)

    if suffix == STREAM_SUFFIX && (room = room_from(gid_param))
      user.rooms.find_by(id: room.id)
    end
  end

  def self.room_from(gid_param)
    begin
      GlobalID::Locator.locate gid_param, only: Room
    rescue ActiveRecord::RecordNotFound
      nil
    end
  end

  def subscribed
    if stream_name = authorized_stream_name
      stream_from stream_name
    else
      reject
    end
  end

  def authorized_stream_name
    stream_name = verified_stream_name_from_params
    stream_name if ActiveSupport.present?(stream_name) && self.class.subscribable_room(current_user, stream_name)
  end
end

# campfire's `app/models/room_streams_are_authorized.rb`, verbatim.
module RoomStreamsAreAuthorized
  def subscribed
    if RoomMessagesChannel.guarded_stream?(verified_stream_name_from_params)
      reject
    else
      super
    end
  end
end

class UnreadRoomsChannel < ApplicationCable::Channel
  def self.stream_name_for(user_id) = "user_#{user_id}_unreads"
  def subscribed = stream_from(self.class.stream_name_for(current_user.id))
end

class ExplodingChannel < ApplicationCable::Channel
  def subscribed = raise("the app raised")
end

# What the emitted boot.rb's generated mixin block does.
Turbo::StreamsChannel.prepend RoomStreamsAreAuthorized

# --- probes ----------------------------------------------------------
FAILURES = []

def check(label, got, want)
  if got == want
    puts "  ok   #{label}"
  else
    FAILURES << label
    puts "  FAIL #{label}: got #{got.inspect}, want #{want.inspect}"
  end
end

Room::ALL[1] = Room.new(1)
Room::ALL[2] = Room.new(2)
MEMBER = User.new(7, [1])          # a member of room 1 only
OUTSIDER = User.new(8, [])

# The identity a `/cable` handshake resolved, and the connection stub
# that carries it. `Dispatch` reads nothing else off a connection.
Identity = Struct.new(:current_user)
Conn = Struct.new(:identity)

# `turbo_stream_from @room, :messages` mints exactly this, and
# `ActionView::ViewHelpers.turbo_stream_from` base64s the JSON of it
# into the page's `signed-stream-name`.
def stream_name(room, suffix) = "#{room.to_gid_param}:#{suffix}"

def signed(name) = "#{Base64.strict_encode64(JSON.generate(name))}--unsigned"

def subscribe(channel_name, params, user)
  identifier = { "channel" => channel_name }.merge(params)
  json = JSON.generate(identifier)
  klass = ActionCable::Channel::Base.lookup(channel_name)
  return :no_such_channel if klass.nil?
  Cable::Dispatch.subscribe(klass, Conn.new(Identity.new(user)), json, identifier)
end

def streams_of(outcome)
  Cable::Dispatch.rejected?(outcome) ? :rejected : outcome.streams
end

# --- the locator, both directions ------------------------------------
# Round-tripped rather than compared to a literal: the mint lives in
# runtime/ruby/rails.rb and the parse in runtime/spinel/global_id_locator.rb,
# and this is the assertion that keeps the two files in step.
check("a minted gid locates its record",
      GlobalID::Locator.locate(Room::ALL[1].to_gid_param, only: Room)&.id, 1)
check("a gid for another model does not locate",
      GlobalID::Locator.locate(GlobalID.param("User", 1), only: Room), nil)
check("a gid from another app does not locate",
      GlobalID::Locator.locate(Base64.urlsafe_encode64_nopad("gid://other/Room/1"), only: Room),
      nil)
check("a param that is not base64 does not locate",
      GlobalID::Locator.locate("!!!!", only: Room), nil)
check("a gid for a missing record raises RecordNotFound",
      (begin
        GlobalID::Locator.locate(GlobalID.param("Room", 99), only: Room)
      rescue ActiveRecord::RecordNotFound
        :raised
      end), :raised)

# --- the guard, which is what item 4 exists for ----------------------
messages = stream_name(Room::ALL[1], "messages")

# THE BYPASS, CLOSED. Same signed stream name, stock channel: campfire's
# prepend refuses it before `super` ever reaches `stream_from`.
check("the stock channel refuses a :messages stream",
      streams_of(subscribe("Turbo::StreamsChannel",
                           { "signed_stream_name" => signed(messages) }, MEMBER)),
      :rejected)

# ... and refuses it for a member too. The point of the guard is that
# the CHANNEL is wrong, not that the user is.
check("the stock channel refuses it even for a member",
      streams_of(subscribe("Turbo::StreamsChannel",
                           { "signed_stream_name" => signed(messages) }, MEMBER)),
      :rejected)

# A name the app did not guard still goes through the stock channel, and
# `super` is what carries it — proof the prepend calls up rather than
# replacing the body.
other = stream_name(Room::ALL[1], "presence")
check("the stock channel still serves an unguarded stream",
      streams_of(subscribe("Turbo::StreamsChannel",
                           { "signed_stream_name" => signed(other) }, MEMBER)),
      [other])

# --- the door that is left ------------------------------------------
check("a member subscribes through RoomMessagesChannel",
      streams_of(subscribe("RoomMessagesChannel",
                           { "signed_stream_name" => signed(messages) }, MEMBER)),
      [messages])

check("a non-member is refused by RoomMessagesChannel",
      streams_of(subscribe("RoomMessagesChannel",
                           { "signed_stream_name" => signed(messages) }, OUTSIDER)),
      :rejected)

# A member of room 1 naming room 2's stream. The room is derived from
# the stream name, so there is nothing to point elsewhere — but the
# membership still has to be checked against the room it names.
check("a member of another room is refused",
      streams_of(subscribe("RoomMessagesChannel",
                           { "signed_stream_name" => signed(stream_name(Room::ALL[2], "messages")) },
                           MEMBER)),
      :rejected)

check("a garbage stream name is refused",
      streams_of(subscribe("RoomMessagesChannel",
                           { "signed_stream_name" => "not-base64--unsigned" }, MEMBER)),
      :rejected)

check("a missing stream name is refused",
      streams_of(subscribe("RoomMessagesChannel", {}, MEMBER)), :rejected)

# --- the rest of the dispatch surface --------------------------------
check("a channel with no signed name streams what it computes",
      streams_of(subscribe("UnreadRoomsChannel", {}, MEMBER)), ["user_7_unreads"])

check("an unregistered channel name resolves to nothing",
      subscribe("Kernel", {}, MEMBER), :no_such_channel)

check("a channel that raises rejects rather than escaping",
      streams_of(subscribe("ExplodingChannel", {}, MEMBER)), :rejected)

# `stream_for`/`broadcast_to` spell the stream name once, through
# `broadcasting_for`. actioncable: name minus "Channel", "::" to ":",
# underscored, then the record's gid param.
check("channel_name follows actioncable's spelling",
      [RoomMessagesChannel.channel_name, Turbo::StreamsChannel.channel_name],
      ["room_messages", "turbo:streams"])
check("broadcasting_for joins the channel name to the record's gid",
      ActionCable::Channel::Base::REGISTRY["UnreadRoomsChannel"]
        .broadcasting_for(Room::ALL[1]),
      "unread_rooms:#{Room::ALL[1].to_gid_param}")

# Channel bodies read `params[:room_id]`; the wire has string keys only.
params = ActionCable::Channel::Parameters.new({ "room_id" => 1 })
check("params read indifferently", [params[:room_id], params["room_id"]], [1, 1])

if FAILURES.empty?
  puts "ALL OK"
else
  puts "#{FAILURES.length} FAILED"
  exit 1
end
