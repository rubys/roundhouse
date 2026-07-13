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
