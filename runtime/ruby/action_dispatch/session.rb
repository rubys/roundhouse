module ActionDispatch
  # Per-app session store. Real-blog uses no session keys, so the
  # struct is empty (no typed fields). It still exposes HWIA-shape
  # shim methods so framework tests calling `@controller.session.length()`
  # compile and pass. Apps with session schema grow this struct in
  # parallel with their controller usage (the typed-targets pipeline
  # picks up new fields via the same scan that drives Flash).
  #
  # Internal `@data` Hash is kept as the storage so future per-app
  # session keys can be threaded through without a runtime rewrite —
  # the shim methods already route through it.
  class Session
    def initialize(other = nil)
      @data = {}
      return if other.nil?
      keys = other.keys
      i = 0
      while i < keys.length
        k = keys[i]
        v = other[k]
        @data[k.to_s] = v
        i += 1
      end
    end

    def [](key)
      k = key.to_s
      return @data[k] if @data.key?(k)
      nil
    end

    def []=(key, value)
      @data[key.to_s] = value
      value
    end

    def fetch(key, default = nil)
      k = key.to_s
      return @data[k] if @data.key?(k)
      default
    end

    def key?(key)
      @data.key?(key.to_s)
    end

    def has_key?(key)
      @data.key?(key.to_s)
    end

    def include?(key)
      @data.key?(key.to_s)
    end

    def delete(key)
      @data.delete(key.to_s)
    end

    def length
      @data.length
    end

    def size
      @data.length
    end

    def empty?
      @data.empty?
    end

    def keys
      @data.keys
    end

    def values
      @data.values
    end

    def each
      keys = @data.keys
      i = 0
      while i < keys.length
        k = keys[i]
        v = @data[k]
        yield k, v
        i += 1
      end
      self
    end

    def to_h
      @data
    end

    def merge(other)
      result = Session.new(to_h)
      other.each do |k, v|
        result[k] = v
      end
      result
    end

    # ── cookie-carried persistence ──────────────────────────────────
    # The whole session rides in a `_session` cookie as url-encoded
    # `k=v&k2=v2` pairs (values are strings; lobsters keeps only string
    # tokens: `u`, `twofa_u`, `redirect_to`, `_csrf_token`). Stateless
    # by design — the dispatch layer owns the restore/persist call
    # sites and compares inbound-vs-outbound encodings to decide
    # whether a Set-Cookie is needed. No signing/encryption — parity
    # with Rails' encrypted CookieStore is a wire format, not a
    # behavior, difference. The codec escapes a fixed set — the
    # format's own delimiters (% & = +) plus the cookie-hostile chars
    # (space ; , ") — via gsub-with-Hash, the same idiom as
    # JsonBuilder's encode_string, so every target's emitter already
    # compiles it. Decode is single-pass (gsub never rescans a
    # replacement), so "%2525" decodes to "%25" exactly once; a code
    # outside the set rides through verbatim — tolerant, matching
    # from_cookie's stale-cookie stance.
    ENCODES = {
      "%" => "%25",
      "&" => "%26",
      "=" => "%3D",
      "+" => "%2B",
      " " => "%20",
      ";" => "%3B",
      "," => "%2C",
      "\"" => "%22",
    }.freeze
    ENCODE_PATTERN = /[%&=+ ;,"]/.freeze

    # `"+" => " "` restores the url-encoding legacy spelling on the
    # inbound side only; our own encoder always writes %20.
    DECODES = {
      "%25" => "%",
      "%26" => "&",
      "%3D" => "=",
      "%2B" => "+",
      "%20" => " ",
      "%3B" => ";",
      "%2C" => ",",
      "%22" => "\"",
      "+" => " ",
    }.freeze
    DECODE_PATTERN = /%25|%26|%3D|%2B|%20|%3B|%2C|%22|\+/.freeze

    # Decode a `_session` cookie value into a Session. Tolerates a
    # missing/garbled cookie by starting empty — a stale cookie shape
    # should mean "logged out", not a 500. A pair whose key decodes
    # empty is dropped; a pair with no `=` keeps its key with an empty
    # value (our encoder always writes the `=`).
    def self.from_cookie(raw)
      session = Session.new
      raw.to_s.split("&").each do |pair|
        parts = pair.to_s.split("=")
        k = Session.cookie_decode(parts[0].to_s)
        v = Session.cookie_decode(parts[1].to_s)
        session[k] = v unless k.empty?
      end
      session
    end

    # Inverse of from_cookie. Deterministic (insertion order), so the
    # dispatcher can compare encodings to detect change.
    def to_cookie
      ks = keys
      ks.map { |k| "#{Session.cookie_encode(k)}=#{Session.cookie_encode(self[k].to_s)}" }.join("&")
    end

    def self.cookie_encode(s)
      s.gsub(ENCODE_PATTERN, ENCODES)
    end

    def self.cookie_decode(s)
      s.gsub(DECODE_PATTERN, DECODES)
    end
  end
end
