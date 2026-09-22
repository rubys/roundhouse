# The `web-push` gem's delivery (3.0.2, campfire's lock) on spinel:
# `WebPush::Request` as the gem has it, and the `WebPush.deliver` the
# façade in runtime/gem_facades.rb leaves failing, redefined here to
# send through it. The cryptography is runtime/web_push_crypto.rb.
#
# THE RUBY FAMILY NEVER LOADS THIS FILE: `project::ruby_runtime_files`
# swaps it for the façade alone, and the façade's stub slot aliases the
# real gem's `payload_send`. The strict targets have no openssl, so the
# façade's failing `deliver` is what they keep.
#
# WHY `Request` IS PORTED AS A CLASS rather than folded into `deliver`:
# campfire prepends `WebPush::PersistentRequest` onto it (config/
# initializers/web_push.rb), and that module is the app's SSRF guard —
# it replaces `#perform` to connect to the address `Push::Subscription`
# resolved and vetted, never to the endpoint's hostname again. Its body
# reads the gem's `@options`, `uri`, `headers`, `body` and
# `verify_response`, so those are the gem's names here too. A delivery
# that skipped the class would send unpinned: the rebinding window the
# app closed on purpose, reopened by the compile.
#
# WHAT IS NOT PORTED, and raises rather than pretending:
# * `vapid: { pem: }` — campfire configures the key pair as the two
#   base64url halves; this package parses no PEM.
# * `proxy:` — spinel's client has no proxy support. campfire's pinned
#   path disables proxies explicitly, which is the path it takes.
#
# ONE DELIBERATE DIFFERENCE: `verify_response` asks the status CODE
# where the gem asks the response CLASS, because packages/net's
# hierarchy has no `HTTPGone`, `HTTPPayloadTooLarge` or
# `HTTPTooManyRequests` — a 410 arrives as a bare `HTTPClientError`. The
# code is what those classes are named for, so the mapping is the gem's.
require_relative "gem_facades"
require_relative "net_http"
require_relative "web_push_crypto"

module WebPush
  class Request
    # Positional, not the gem's `(message:, subscription:, vapid:,
    # **options)`: `deliver` below is the only caller, and the gem's
    # constructor is not something the app writes. `@options` keeps the
    # gem's shape — a Symbol-keyed Hash with its defaults merged — since
    # the app's prepended `perform` reads it.
    def initialize(message, endpoint, p256dh, auth, vapid, options)
      @endpoint = endpoint
      @uri = URI.parse(endpoint)
      @payload = message.empty? ? "" : Encryption.encrypt(message, p256dh, auth)
      @vapid_options = vapid
      @options = options
    end

    def perform
      raise NotImplementedError, "web-push: proxy: is not supported on this target" unless @options[:proxy].nil?
      http = Net::HTTP.new(uri.host, uri.port)
      http.use_ssl = true
      http.open_timeout = @options[:open_timeout] unless @options[:open_timeout].nil?
      http.read_timeout = @options[:read_timeout] unless @options[:read_timeout].nil?

      req = Net::HTTP::Post.new(uri.request_uri, headers)
      req.body = body

      resp = http.request(req)
      verify_response(resp)

      resp
    end

    def proxy_options
      []
    end

    def headers
      headers = {}
      headers["Content-Type"] = "application/octet-stream"
      headers["Ttl"] = ttl
      headers["Urgency"] = urgency

      unless @payload.empty?
        headers["Content-Encoding"] = "aes128gcm"
        headers["Content-Length"] = @payload.bytesize.to_s
      end

      headers["Authorization"] = build_vapid_header if vapid?

      headers
    end

    def build_vapid_header
      raise NotImplementedError, "web-push: vapid pem: is not supported on this target" if @vapid_options.key?(:pem)
      Vapid.header(audience, subject, @vapid_options.fetch(:public_key, ""), @vapid_options.fetch(:private_key, ""), expiration)
    end

    def body
      @payload
    end

    private

    def uri
      @uri
    end

    def ttl
      @options.fetch(:ttl).to_s
    end

    def urgency
      @options.fetch(:urgency).to_s
    end

    def audience
      uri.scheme + "://" + uri.host
    end

    # The gem's twelve hours. The option is an Integer there; campfire
    # never passes it, and every other VAPID value is a String.
    def expiration
      @vapid_options.fetch(:expiration, "43200").to_i
    end

    def subject
      @vapid_options.fetch(:subject, "mailto:sender@example.com")
    end

    def vapid?
      !@vapid_options.empty?
    end

    def verify_response(resp)
      code = resp.code
      if code == "410"
        raise ExpiredSubscription.new(resp, uri.host)
      elsif code == "404"
        raise InvalidSubscription.new(resp, uri.host)
      elsif code == "401" || code == "403" || (code == "400" && resp.message == "UnauthorizedRegistration")
        raise Unauthorized.new(resp, uri.host)
      elsif code == "413"
        raise PayloadTooLarge.new(resp, uri.host)
      elsif code == "429"
        raise TooManyRequests.new(resp, uri.host)
      elsif code.start_with?("5")
        raise PushServiceError.new(resp, uri.host)
      elsif !code.start_with?("2")
        raise ResponseError.new(resp, uri.host)
      end
      nil
    end
  end

  # The gem's `payload_send` past the stub slot: its defaults (four
  # weeks' TTL, "normal" urgency) under the options the caller passed —
  # the gem merges only the keys given, and an absent key reads nil
  # either way, so passing all four is the same Hash to every reader.
  def self.deliver(message, endpoint, p256dh, auth, vapid, connection, urgency, endpoint_ip)
    options = {
      ttl: 60 * 60 * 24 * 7 * 4,
      urgency: urgency.nil? ? "normal" : urgency,
      endpoint_ip: endpoint_ip,
      connection: connection,
    }
    Request.new(message, endpoint, p256dh, auth, vapid, options).perform
    ""
  end
end
