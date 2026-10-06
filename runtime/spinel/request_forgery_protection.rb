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
        req.env.fetch("HTTP_ORIGIN", "").to_s, req.host.to_s)
      expected = session[:_csrf_token].to_s
      return true if AuthenticityToken.valid?(
        Params.str(params, "authenticity_token", ""), expected)
      AuthenticityToken.valid?(
        req.env.fetch("HTTP_X_CSRF_TOKEN", "").to_s, expected)
    end
  end

  module RequestForgeryProtection
    # An absent Origin passes (some agents omit it); `null` — a sandboxed
    # frame, a privacy redirect — does not. Otherwise the header's host
    # must be the request's own Host.
    def self.valid_origin?(origin, host)
      return true if origin.empty?
      return false if origin == "null"
      at = origin.index("://")
      return false if at.nil?
      origin[at + 3, origin.length].to_s == host
    end

    # Action Cable's `allow_request_origin?`, the check a `/cable`
    # handshake passes before any connection code runs. Rails' defaults
    # (`allow_same_origin_as_host` true, and `allowed_request_origins`
    # set to `/https?:\/\/localhost:\d+/` in development only):
    #
    # * the Origin must name the request's own Host -- compared by host,
    #   as `valid_origin?` above is, and for its reason;
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
    def self.cable_origin_allowed?(origin, host, development)
      return false if origin.empty?
      return true if valid_origin?(origin, host)
      development && origin.match?(/\Ahttps?:\/\/localhost:\d+\z/)
    end

    # Constant-time unmask + compare (AuthenticityToken).
    def self.token_matches?(given, expected)
      ActionController::AuthenticityToken.valid?(given, expected)
    end
  end
end
