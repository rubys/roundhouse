# CRuby-only session persistence + CSRF token generation.
#
# Follows the CookieJar precedent (action_controller_cookies.rb): the
# session is a CRuby-target feature until other targets take on
# lobsters, so everything here lives in the Ruby overlay rather than
# the shared runtime/ (which transpiles to every target). The shared
# runtime contributes only the ActionDispatch::Session data object,
# Base#reset_session, and the empty-string form_authenticity_token
# default this file overrides.
#
# Storage model: cookie-carried, stateless — the whole session rides
# in a `_session` cookie as url-encoded `k=v&k2=v2` pairs (values are
# strings; lobsters keeps only string tokens in the session: `u`,
# `twofa_u`, `redirect_to`, and our `_csrf_token`). Stateless matters
# here twice over: the one-shot CGI probe path spawns a fresh process
# per request, and the benchmark's Puma path must not depend on
# in-process state either. No signing/encryption — the benchmark
# harness is the only client, and it replays cookies verbatim; parity
# with Rails' encrypted CookieStore is a wire format, not behavior,
# difference. The dispatch layer (main.rb) owns the restore/persist
# call sites.

require "securerandom"

module ActionDispatch
  class Session
    # Decode a `_session` cookie value into a Session. Tolerates a
    # missing/garbled cookie by starting empty — the benchmark never
    # sends one on first contact, and a stale cookie shape after a
    # redeploy should mean "logged out", not a 500.
    def self.from_cookie(raw)
      data = {}
      raw.to_s.split("&").each do |pair|
        eq = pair.index("=")
        next if eq.nil?
        k = CgiIo.url_decode(pair[0, eq])
        v = CgiIo.url_decode(pair[(eq + 1)..].to_s)
        data[k] = v unless k.empty?
      end
      Session.new(data)
    end

    # Inverse of from_cookie. Deterministic (insertion order), so
    # dispatch can compare inbound-vs-outbound encodings to decide
    # whether a Set-Cookie is needed at all.
    def to_cookie
      to_h.map { |k, v| "#{CgiIo.url_encode(k)}=#{CgiIo.url_encode(v.to_s)}" }.join("&")
    end
  end
end

module ActionView
  module ViewHelpers
    # Session-backed lazy CSRF token, overriding the shared runtime's
    # empty-string default. Lazy generation keeps token creation (and
    # therefore session-cookie emission) scoped to requests that
    # actually render a csrf-consuming helper — pages without forms or
    # csrf_meta_tags don't grow a session. Reaches the session through
    # ActionController::Current because view helpers are module
    # functions with no controller context (same pattern as
    # Current.request).
    def self.form_authenticity_token
      session = ActionController::Current.session
      return "" if session.nil?
      token = session[:_csrf_token]
      if token.nil?
        token = SecureRandom.urlsafe_base64(32)
        session[:_csrf_token] = token
      end
      token.to_s
    end
  end
end
