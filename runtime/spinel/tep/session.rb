# Tep::Session -- string-keyed string store, persisted in a signed
# cookie. Format: `urlencoded_payload.hexhmac` where the signature
# covers exactly the urlencoded payload. Forgery-resistant given a
# strong secret; payload is *visible* to clients (not encrypted).
#
# To enable: set `Tep.session_secret` to a long random string at app
# load time (e.g. `Tep.session_secret = ENV.fetch("TEP_SESSION_SECRET")`).
# When unset, sessions silently no-op (read-only Bag, no Set-Cookie).
module Tep
  COOKIE_NAME = "tep.session"

  class Session
    attr_accessor :data, :dirty

    def initialize
      @data  = Tep.str_hash
      @dirty = false
    end

    # Spinel doesn't dispatch user-defined `[]` / `[]=` on user
    # classes -- and emitting them at all forces those methods to
    # default-typed mrb_int params for callers we don't have, which
    # mismatches the underlying String/String slots. So Session
    # exposes only named methods; the translator rewrites
    # `session[k] = v` to `session.set(k, v)` and `session[k]` to
    # `session.get(k)` for source compatibility with Sinatra.
    def get(k);    @data[k];                          end
    def set(k, v); @data[k] = v; @dirty = true;       end
    def has?(k);   @data.key?(k);                     end
    def length;    @data.length;                      end
    def clear;     @data = Tep.str_hash; @dirty = true; end

    # Verify + decode an inbound cookie value. Returns true on
    # success (data populated), false on missing / tampered.
    def load_from(cookie_value, secret)
      if cookie_value.length == 0 || secret.length == 0
        return false
      end
      dot = cookie_value.rindex(".")
      if dot < 0
        return false
      end
      payload = cookie_value[0, dot]
      sig     = cookie_value[dot + 1, cookie_value.length - dot - 1]
      expect  = Sock.sphttp_hmac_sha256_hex(secret, payload)
      if !Tep.timing_safe_eq(sig, expect)
        return false
      end
      Url.parse_query(payload).each do |k, v|
        @data[k] = v
      end
      true
    end

    # Serialize + sign for the response cookie. Caller decides when
    # to call this (typically only when @dirty).
    def to_cookie_value(secret)
      payload = ""
      first = true
      @data.each do |k, v|
        if !first
          payload = payload + "&"
        end
        payload = payload + Url.escape(k) + "=" + Url.escape(v)
        first = false
      end
      payload + "." + Sock.sphttp_hmac_sha256_hex(secret, payload)
    end
  end

  # Constant-time string equality. Avoids leaking the matching prefix
  # length via early-exit timing. spinel doesn't have a stdlib
  # crypto-safe compare, so we roll our own.
  def self.timing_safe_eq(a, b)
    if a.length != b.length
      return false
    end
    diff = 0
    i = 0
    while i < a.length
      diff = diff | (a.bytes[i] ^ b.bytes[i])
      i += 1
    end
    diff == 0
  end
end
