# frozen_string_literal: true
# Sponge façade — lobsters' vendored HTTP client (extras/sponge.rb)
# drives Net::HTTP, Resolv, IPAddr, and OpenSSL, none of which exist
# under spinel AOT until the stdlib spin packages land. Every consumer
# is submit-time or write-path (story URL fetching, gravatar, keybase/
# github/pushover/diff_bot), so the scaffold base ships this raising
# stand-in at the same require path (`app/models/sponge.rb`) and the
# require graph is unchanged; the CRuby tree — where the real stdlib
# exists and the vendored source runs as written — restores the
# verbatim emit (see emit::ruby::library::restore_extras_facades).
# Same contract as runtime/gem_facades.rb: everything compiles, every
# runtime hit raises loudly, returns carry the real shapes consumers
# chain on.
require "securerandom"
require_relative "../../runtime/gem_facades"

class Sponge
  MAX_TIME = 60
  MAX_DNS_TIME = 5

  # The slice of Net::HTTPResponse consumers actually touch on a fetch
  # result: `.body`, `.code`, and `res["content-type"]`.
  class Response
    def body
      GemFacade.fail!("Sponge::Response#body")
      ""
    end

    def code
      GemFacade.fail!("Sponge::Response#code")
      ""
    end

    def [](_name)
      GemFacade.fail!("Sponge::Response#[]")
      ""
    end
  end

  attr_accessor :debug, :last_res, :timeout, :ssl_verify

  def self.fetch(url, headers = {}, limit = 10)
    GemFacade.fail!("Sponge.fetch")
    Response.new
  end

  def initialize
    @debug = false
    @timeout = MAX_TIME
    @ssl_verify = true
  end

  def set_cookie(host, name, val)
    GemFacade.fail!("Sponge#set_cookie")
    nil
  end

  def cookies(host)
    GemFacade.fail!("Sponge#cookies")
    ""
  end

  def fetch(url, method = :get, fields = nil, raw_post_data = nil, headers = {}, limit = 10)
    GemFacade.fail!("Sponge#fetch")
    Response.new
  end

  def get(url)
    GemFacade.fail!("Sponge#get")
    Response.new
  end

  def post(url, fields)
    GemFacade.fail!("Sponge#post")
    Response.new
  end
end

# OpenSSL::Random — entropy for Utils.random_str (short-ids, session
# and reset tokens; short-id generation runs on the replay's story
# and comment submits, so this façade is REAL, not raising).
# SecureRandom.random_bytes draws from spinel's native CSPRNG,
# backed by OS entropy rather than Kernel#rand's seedable PRNG. The
# implementation module is `RandomSource` with a constant alias
# because a module literally named Random collides with the builtin
# PRNG's C typedef (spinel#2455). The CRuby tree restores the real
# stdlib (see restore_extras_facades) and never sees this.
module OpenSSL
  module RandomSource
    def self.random_bytes(n)
      # Multi-draw SDK buffers can be UTF-8; OpenSSL always returns binary bytes.
      SecureRandom.random_bytes(n).b
    end
  end

  Random = RandomSource
end
