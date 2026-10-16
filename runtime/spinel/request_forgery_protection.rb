# frozen_string_literal: true
# Rails' request forgery check: Origin / Action Cable helpers, and the
# ruby-family reopen of `verified_request?` that adds the Origin rule
# the shared Base cannot (it has no Request). Token matching is
# `AuthenticityToken.valid?` — masked XOR tokens, not a verbatim
# session string.
#
# Required from BOTH ruby-family boots, after the controller runtime.
module ActionController
  class Base
    def verified_request?
      return true unless ActionController.forgery_flag
      req = ActionController::Current.request
      # No parked Request (unit-style `controller.process_action`): fall
      # back to Base's `@request_method` rule. Empty/GET/HEAD pass; a
      # mutating verb still needs a valid token. Origin cannot be
      # checked without a Request — that is not a weaken of the
      # parked-request path below.
      if req.nil?
        verb = @request_method.to_s
        return true if verb == "" || verb == "GET" || verb == "HEAD"
        expected = session[:_csrf_token].to_s
        return true if AuthenticityToken.valid?(
          Params.str(params, "authenticity_token", ""), expected)
        return AuthenticityToken.valid?(csrf_header_token, expected)
      end
      verb = req.request_method
      return true if verb == "GET" || verb == "HEAD"
      return false unless RequestForgeryProtection.valid_origin?(
        req.env.fetch("HTTP_ORIGIN", "").to_s, req.base_url)
      # Rails main's Fetch Metadata check, ahead of any token: the
      # browser's own `Sec-Fetch-Site` vouches for a same-origin or
      # same-site request, and a cross-site one is refused (Rails admits
      # it only from `forgery_protection_trusted_origins`, which is not
      # modeled — empty, Rails' default). What a MISSING or other value
      # means is the strategy's: `header_only` (the 8.2 default) passes
      # a missing header on plain http only, `header_or_legacy_token`
      # falls back to the token below. Not modeled: `force_ssl`'s
      # `secure_protocol`, which also refuses a missing header on http;
      # such an app redirects plain http before it gets here.
      site = req.env.fetch("HTTP_SEC_FETCH_SITE", "").to_s.downcase
      return true if site == "same-origin" || site == "same-site"
      return false if site == "cross-site"
      if Rails.application.forgery_protection_verification_strategy == "header_only"
        return site == "" && !req.ssl?
      end
      expected = session[:_csrf_token].to_s
      return true if AuthenticityToken.valid?(
        Params.str(params, "authenticity_token", ""), expected)
      AuthenticityToken.valid?(
        req.env.fetch("HTTP_X_CSRF_TOKEN", "").to_s, expected)
    end
  end

  module RequestForgeryProtection
    # actionpack's `valid_request_origin?`: an absent Origin passes (some
    # agents omit it); `null` — a sandboxed frame, a privacy redirect —
    # does not. Otherwise the header must BE the request's base URL,
    # scheme included: `http://chat.example.com` posting to the https
    # site is another origin, which a host-only comparison let through.
    def self.valid_origin?(origin, base_url)
      return true if origin.empty?
      return false if origin == "null"
      origin == base_url
    end

    # The base URL a request arrived on, from its Host header and the
    # same TLS evidence `ActionDispatch::Request#ssl?` reads (`HTTPS=on`
    # from the server, `X-Forwarded-Proto` from a proxy that terminated
    # TLS in front of it, else the scheme Rack itself reports in
    # `rack.url_scheme`, the one TLS key the Rack spec requires), the
    # scheme's standard port dropped as `Request#base_url` drops it.
    def self.base_url_for(host, https, forwarded_proto, url_scheme)
      forwarded = forwarded_proto.split(",").first.to_s.strip.downcase
      tls = https == "on" || (forwarded.empty? ? url_scheme.downcase == "https" : forwarded == "https")
      default = tls ? ":443" : ":80"
      bare = host.end_with?(default) ? host[0, host.length - default.length].to_s : host
      (tls ? "https://" : "http://") + bare
    end

    # The origin Action Cable compares a handshake's Origin with:
    # `"#{proto}://#{env['HTTP_HOST']}"`, the scheme from the same TLS
    # evidence `base_url_for` reads and the Host header as sent. Unlike
    # `base_url_for`, a standard port the client wrote stays: Rails
    # builds this string from the raw header, so `Host: example.com:443`
    # is not `https://example.com`.
    def self.host_origin_for(host, https, forwarded_proto, url_scheme)
      tls = base_url_for(host, https, forwarded_proto, url_scheme).start_with?("https://")
      (tls ? "https://" : "http://") + host
    end

    # Action Cable's `allow_request_origin?` (actioncable 8.1.4,
    # `ActionCable::Connection::Base`), the check a `/cable` handshake
    # passes before any connection code runs, over the app's
    # `config.action_cable` settings as `Rails.application` answers them
    # (Rails' defaults in runtime/ruby/rails.rb, the app's assignments
    # lifted at ingest):
    #
    # * `disable_request_forgery_protection` allows every handshake;
    # * with `allow_same_origin_as_host` (default true) the Origin may be
    #   `host_origin`, the request's own scheme and Host header;
    # * otherwise, or besides, it must be `===` one of
    #   `allowed_request_origins` (a String equal to it, a Regexp
    #   matching it anywhere): `action_cable_origin_listed?`, which in
    #   development defaults to any `localhost` port.
    #
    # An ABSENT Origin ("" here) is refused unless the check is disabled:
    # Rails compares `env["HTTP_ORIGIN"]`, nil, which equals no host and
    # no listed String and matches no Regexp.
    def self.cable_origin_allowed?(origin, host_origin)
      app = Rails.application
      return true if app.action_cable_disable_request_forgery_protection
      return false if origin.empty?
      return true if app.action_cable_allow_same_origin_as_host && origin == host_origin
      app.action_cable_origin_listed?(origin)
    end

    # Constant-time unmask + compare (AuthenticityToken).
    def self.token_matches?(given, expected)
      ActionController::AuthenticityToken.valid?(given, expected)
    end
  end
end
