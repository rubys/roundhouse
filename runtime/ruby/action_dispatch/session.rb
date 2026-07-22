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
    # behavior, difference. Codec is hand-rolled byte-loop percent
    # encoding so this file stays portable to every target.

    # Decode a `_session` cookie value into a Session. Tolerates a
    # missing/garbled cookie by starting empty — a stale cookie shape
    # should mean "logged out", not a 500.
    def self.from_cookie(raw)
      session = Session.new
      pairs = raw.to_s.split("&")
      i = 0
      while i < pairs.length
        pair = pairs[i].to_s
        eq = pair.index("=")
        unless eq.nil?
          k = Session.cookie_decode(pair[0, eq].to_s)
          v = Session.cookie_decode(pair[eq + 1, pair.length].to_s)
          session[k] = v unless k.empty?
        end
        i += 1
      end
      session
    end

    # Inverse of from_cookie. Deterministic (insertion order), so the
    # dispatcher can compare encodings to detect change.
    def to_cookie
      out = ""
      ks = keys
      i = 0
      while i < ks.length
        k = ks[i]
        out += "&" if i > 0
        out += Session.cookie_encode(k) + "=" + Session.cookie_encode(fetch(k, "").to_s)
        i += 1
      end
      out
    end

    def self.cookie_encode(s)
      hex = "0123456789ABCDEF"
      out = ""
      i = 0
      while i < s.length
        c = s[i, 1].to_s
        o = c.ord
        if (o >= 48 && o <= 57) || (o >= 65 && o <= 90) || (o >= 97 && o <= 122) ||
           c == "-" || c == "_" || c == "." || c == "~"
          out += c
        else
          out += "%" + hex[o / 16, 1].to_s + hex[o % 16, 1].to_s
        end
        i += 1
      end
      out
    end

    def self.cookie_decode(s)
      out = ""
      i = 0
      while i < s.length
        c = s[i, 1].to_s
        if c == "%" && i + 2 < s.length
          hi = Session.hex_val(s[i + 1, 1].to_s)
          lo = Session.hex_val(s[i + 2, 1].to_s)
          if hi >= 0 && lo >= 0
            out += (hi * 16 + lo).chr
            i += 3
          else
            out += c
            i += 1
          end
        elsif c == "+"
          out += " "
          i += 1
        else
          out += c
          i += 1
        end
      end
      out
    end

    def self.hex_val(c)
      o = c.length > 0 ? c.ord : 0
      return o - 48 if o >= 48 && o <= 57
      return o - 55 if o >= 65 && o <= 70
      return o - 87 if o >= 97 && o <= 102
      -1
    end
  end
end
