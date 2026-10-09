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

    # Action Cable's `allow_request_origin?`, the check a `/cable`
    # handshake passes before any connection code runs. Rails' defaults
    # (`allow_same_origin_as_host` true, and `allowed_request_origins`
    # set to `/https?:\/\/localhost:\d+/` in development only):
    #
    # * the Origin must be the request's own base URL --
    #   `"#{proto}://#{env['HTTP_HOST']}" == env["HTTP_ORIGIN"]`, scheme
    #   included, as `valid_origin?` above compares it;
    # * in development, any `localhost` port is allowed besides;
    # * an ABSENT Origin is refused. This is where the socket check and
    #   the form check differ in Rails too: `env["HTTP_ORIGIN"]` is nil,
    #   and nil equals no allowed origin.
    #
    # Not modeled: `config.action_cable.allowed_request_origins` and
    # `disable_request_forgery_protection` as an app sets them (ingest
    # reads no `config.action_cable` key). The localhost pattern is
    # anchored, where Rails' `===` is not, so `http://localhost:1.evil`
    # passes Rails in development and fails here.
    def self.cable_origin_allowed?(origin, base_url, development)
      return false if origin.empty?
      return true if valid_origin?(origin, base_url)
      development && origin.match?(/\Ahttps?:\/\/localhost:\d+\z/)
    end

    # Constant-time unmask + compare (AuthenticityToken).
    def self.token_matches?(given, expected)
      ActionController::AuthenticityToken.valid?(given, expected)
    end
  end
end
