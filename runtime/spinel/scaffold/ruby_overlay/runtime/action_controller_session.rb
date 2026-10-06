# CRuby overlay session codec (`from_cookie` / `to_cookie`). CSRF
# minting lives in `runtime/ruby/action_controller/authenticity_token.rb`.
#
# Storage model: cookie-carried, stateless — the whole session rides
# in a `_session` cookie as url-encoded `k=v&k2=v2` pairs. HMAC signing
# of that cookie is `runtime/signed_cookies.rb` on the dispatch path.
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
