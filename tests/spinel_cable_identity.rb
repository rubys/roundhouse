#!/usr/bin/env ruby
# The SPINEL lane's /cable connection identity, exercised.
#
# Loads `runtime/spinel/cable.rb` directly — no emit, no server, no
# socket: `ruby tests/spinel_cable_identity.rb .` reproduces it by hand.
#
# THE SIBLING OF `overlay_cable_identity.rb`, DELIBERATELY NOT SHARED
# WITH IT. Both lanes must refuse the same handshakes, and the probes
# below are the overlay's probe-for-probe. They cannot be one file
# because the two lanes reach the app's connection class by different
# means: the overlay asks `defined?(ApplicationCable::Connection)`, which
# is reflection a static target has no lane for, and this one is handed
# an eager arm by `project::apply_cable_connection`. The behaviour is the
# contract; the resolution mechanism is not.
#
# BOTH ARMS OF THE FACTORY RUN HERE. The file on disk carries the
# DEFAULT arm (an app with no `app/channels/`, which must still connect
# — anonymously), so that is probed first, against the real file. The
# generated arm is then installed exactly as the generator writes it,
# and `tests/spinel_cable_identity.rs` asserts the generator writes that
# same text, so the two halves cannot drift apart silently.
#
# The app connection below is campfire's `app/channels/application_cable/
# connection.rb` AS EMITTED, plus the `Authentication::SessionLookup` it
# includes. That is the point of the test: `Cable.identify` must work by
# RUNNING THE APP'S `connect`, so the fixture has to be the app's code
# rather than a restatement of what the runtime happens to do — and it
# has to be the code that actually reaches the tree, which is not quite
# the source (ingest drops `identified_by :current_user`).
root = ARGV[0] or abort "usage: spinel_cable_identity.rb <repo-root>"
root = File.expand_path(root)

require "json"
require "base64"

# --- what boot.rb has standing before cable.rb loads -----------------
module Rails
  Application = Struct.new(:secret_key_base)
  def self.application = @app ||= Application.new("a" * 64)
end

load "#{root}/runtime/spinel/message_digest_cruby.rb"
load "#{root}/runtime/ruby/action_controller/message_verifier.rb"
load "#{root}/runtime/ruby/action_controller/cookies.rb"

# THE TRANSPORT IS STUBBED TO LOAD, NOT TO RUN — the same rule
# `spinel_cable_channel.rb` follows. `cable.rb` subclasses
# `Tep::WebSocket::Handler` in two class bodies, so that constant has to
# exist for the file to define; the identity path never touches a
# driver, an fd or the broadcast table. Left EMPTY rather than
# faked, so a probe that ever does reach the transport dies with a
# NameError naming the constant instead of passing against a stand-in
# that agrees with everything.
module Tep
  module WebSocket
    class Handler; end
  end
end

load "#{root}/runtime/spinel/action_cable.rb"   # requires cable.rb

puts "vendor ruby #{RUBY_VERSION}"

# --- the app half: campfire's own connection -------------------------
User = Struct.new(:id, :name)

class Session
  STORE = {}
  def self.find_by(token:) = STORE[token]
  attr_reader :user
  def initialize(user) = @user = user
end
Session::STORE["good-token"] = Session.new(User.new(7, "Jason"))

module Authentication
  module SessionLookup
    def find_session_by_cookie
      if token = cookies.signed[:session_token]
        Session.find_by(token: token)
      end
    end
  end
end

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

# `Cable.identify` takes tep's request object and reads `.cookies` off
# it — a String->String hash of already-parsed cookies, which is what
# `Main.dispatch` hands `ActionController::CookieJar` for a controller.
# The WebSocket upgrade is an ordinary HTTP GET, so it is the same jar
# over the same header.
FakeReq = Struct.new(:cookies)

def req_with(cookie_value)
  FakeReq.new(cookie_value.nil? ? {} : { "session_token" => cookie_value })
end

# The jar signs on the way out, so the fixture cookie is signed exactly
# the way the app's own `sessions_controller` writes it.
def signed_value(token)
  jar = ActionController::CookieJar.new({})
  jar.signed[:session_token] = token
  jar["session_token"]
end

# --- the DEFAULT arm: an app with no channels ------------------------
# Probed FIRST, and against the file as it ships. An app with no
# `app/channels/application_cable/connection.rb` — the blog fixture —
# must still connect: Turbo Stream fan-out predates identity and cannot
# be made to depend on it. Anonymous is not refused.
anon = Cable.identify(req_with(nil))
check("an app with no ApplicationCable connects anonymously",
      anon.nil?, false)
check("an anonymous connection identifies nobody",
      anon && anon.current_user, nil)

# --- the GENERATED arm ------------------------------------------------
# campfire's emitted connection. `identified_by :current_user` is NOT
# here because ingest drops it (it defines an accessor from a computed
# name); the runtime's `attr_accessor :current_user` is what answers
# `self.current_user =` below. A fixture carrying the source's line
# would define the accessor itself and pass against a runtime that
# provided nothing — which is exactly what happened on the sibling lane,
# until a real socket found it.
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

# VERBATIM what `project::apply_cable_connection` splices between the
# `generated: cable-connection` markers. Pinned from the Rust side by
# `tests/spinel_cable_identity.rs`, so a change to the generator that
# did not change this file fails there rather than leaving this test
# quietly exercising a shape nothing emits.
module Cable
  def self.build_connection(cookies)
    ApplicationCable::Connection.new(cookies)
  end
end

identified = Cable.identify(req_with(signed_value("good-token")))
check("a valid session identifies the user",
      identified && identified.current_user&.name, "Jason")

# Every refusal below is a 401 answered on the handshake, NOT an
# anonymous socket and not a socket that opens and goes quiet.
check("no cookie at all is refused", Cable.identify(req_with(nil)), nil)
check("an unknown token is refused",
      Cable.identify(req_with(signed_value("no-such"))), nil)

# A signature is what makes the token unforgeable; both halves matter.
tampered = signed_value("good-token").sub(/.\z/) { |c| c == "x" ? "y" : "x" }
check("a tampered signature is refused", Cable.identify(req_with(tampered)), nil)
check("an UNSIGNED token is refused",
      Cable.identify(req_with("good-token")), nil)

if FAILURES.empty?
  puts "ALL OK"
else
  puts "#{FAILURES.length} FAILED"
  exit 1
end
