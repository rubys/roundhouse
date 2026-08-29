#!/usr/bin/env ruby
# The CRuby overlay's /cable connection identity, exercised.
#
# Loads the overlay files directly, in boot.rb's order — no emit, no
# server: `ruby tests/overlay_cable_identity.rb .` reproduces it by hand.
#
# The app connection below is campfire's, verbatim from
# `app/channels/application_cable/connection.rb` plus the
# `Authentication::SessionLookup` it includes. That is the point of the
# test: `Cable.identify` must work by RUNNING THE APP'S `connect`, so
# the fixture has to be the app's code rather than a restatement of what
# the runtime happens to do.
root = ARGV[0] or abort "usage: overlay_cable_identity.rb <repo-root>"
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
load "#{root}/runtime/spinel/cgi_io.rb"

# action_cable.rb's one require; the transport half is not under test.
$LOADED_FEATURES << File.expand_path(
  "#{root}/runtime/spinel/scaffold/ruby_overlay/runtime/broadcasts.rb"
)
module Broadcasts
  LOG = []
  TRANSPORTS = []
end

load "#{root}/runtime/spinel/scaffold/ruby_overlay/runtime/action_cable.rb"

# cable.rb requires nio4r and websocket-driver at the top, and the unit
# job installs neither — they are the REACTOR's dependencies, and the
# identity path never reaches the reactor: `Cable.identify` resolves a
# cookie jar and runs the app's `connect`, all before the hijack that
# would build a Connection.
#
# So the features are marked loaded and left EMPTY rather than stubbed.
# An empty `NIO` has no `Selector` and an empty `WebSocket` has no
# `Driver`, so a future probe that does reach the reactor dies with a
# NameError naming the constant, instead of passing against a
# stand-in that agrees with everything. Installing the gems here would
# put a native build on every unit run to exercise code this file does
# not test.
%w[nio websocket/driver].each do |feature|
  $LOADED_FEATURES << "#{feature}.rb"
end
module NIO; end
module WebSocket; end

load "#{root}/runtime/spinel/scaffold/ruby_overlay/cable.rb"

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

module ApplicationCable
  class Connection < ActionCable::Connection::Base
    include Authentication::SessionLookup

    identified_by :current_user

    def connect
      self.current_user = find_verified_user
    end

    private
      def find_verified_user
        if verified_session = find_session_by_cookie
          verified_session.user
        else
          reject_unauthorized_connection
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

def env_with(cookie) = { "HTTP_COOKIE" => cookie, "PATH_INFO" => "/cable" }

# The jar signs on the way out, so the fixture cookie is signed exactly
# the way the app's own `sessions_controller` writes it.
def signed_cookie(token)
  jar = ActionController::CookieJar.new({})
  jar.signed[:session_token] = token
  "session_token=#{jar["session_token"]}"
end

identified = Cable.identify(env_with(signed_cookie("good-token")))
check("a valid session identifies the user",
      identified && identified.current_user&.name, "Jason")

# Every refusal below is a 401 at config.ru, NOT an anonymous socket.
check("no cookie at all is refused", Cable.identify(env_with(nil)), nil)
check("an unknown token is refused",
      Cable.identify(env_with(signed_cookie("no-such"))), nil)

# A signature is what makes the token unforgeable; both halves matter.
tampered = signed_cookie("good-token").sub(/.\z/) { |c| c == "x" ? "y" : "x" }
check("a tampered signature is refused", Cable.identify(env_with(tampered)), nil)
check("an UNSIGNED token is refused",
      Cable.identify(env_with("session_token=good-token")), nil)

# An app with no channels at all must keep fanning out Turbo streams:
# stream fan-out predates identity and does not depend on it.
Object.send(:remove_const, :ApplicationCable)
check("an app with no ApplicationCable connects anonymously",
      Cable.identify(env_with(nil)), Cable::NO_IDENTITY)

if FAILURES.empty?
  puts "ALL OK"
else
  puts "#{FAILURES.length} FAILED"
  exit 1
end
