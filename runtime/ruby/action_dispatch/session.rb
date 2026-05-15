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
  #
  # Cookie envelope: `load_from(cookie_value, secret)` and
  # `to_cookie_value(secret)` wrap the in-memory hash in an
  # HMAC-SHA-256 signed payload (Path 3a of the Tep transport
  # adoption — sphttp owns the signing primitive, this class owns
  # what the payload means). Format mirrors tep: urlencoded payload +
  # "." + hex hmac. `@dirty` flips on any mutation; the dispatch
  # layer skips the Set-Cookie write when clean.
  class Session
    attr_reader :dirty

    def initialize(other = nil)
      @data  = {}
      @dirty = false
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
      @dirty = true
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
      k = key.to_s
      out = @data.delete(k)
      @dirty = true unless out.nil?
      out
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

    # Verify + decode an inbound signed cookie. Returns true on
    # success (data populated, dirty flag stays false), false on
    # missing / empty / malformed / tampered. Mirrors tep::Session
    # so the wire format is interoperable.
    def load_from(cookie_value, secret)
      return false if cookie_value.nil? || cookie_value.length == 0
      return false if secret.nil? || secret.length == 0
      dot = cookie_value.rindex(".")
      return false if dot.nil? || dot < 0
      payload = cookie_value[0, dot]
      sig     = cookie_value[(dot + 1)..]
      expect  = Sock.sphttp_hmac_sha256_hex(secret, payload)
      return false unless Session._timing_safe_eq(sig, expect)
      Session._parse_query(payload).each do |k, v|
        @data[k] = v
      end
      true
    end

    # Serialize + sign for the response cookie. Returns the value to
    # set as the Set-Cookie payload. Caller checks `dirty` first.
    def to_cookie_value(secret)
      payload = ""
      first = true
      @data.each do |k, v|
        payload = payload + "&" unless first
        payload = payload + Session._url_escape(k.to_s) + "=" + Session._url_escape(v.to_s)
        first = false
      end
      payload + "." + Sock.sphttp_hmac_sha256_hex(secret, payload)
    end

    # ── helpers ─────────────────────────────────────────────────────
    # Class-method form (not private — spinel's name resolver finds
    # these from anywhere; private semantics aren't enforced at the
    # transpile layer anyway).

    def self._timing_safe_eq(a, b)
      return false if a.length != b.length
      diff = 0
      i = 0
      while i < a.length
        diff = diff | (a.bytes[i] ^ b.bytes[i])
        i += 1
      end
      diff == 0
    end

    def self._url_escape(s)
      out = ""
      i = 0
      while i < s.length
        c = s[i]
        if (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") ||
           (c >= "0" && c <= "9") || c == "-" || c == "." ||
           c == "_" || c == "~"
          out = out + c
        else
          b = c.bytes[0]
          hi = b / 16
          lo = b % 16
          out = out + "%" + _hex_char(hi) + _hex_char(lo)
        end
        i += 1
      end
      out
    end

    def self._url_unescape(s)
      out = ""
      i = 0
      n = s.length
      while i < n
        c = s[i]
        if c == "+"
          out = out + " "
          i += 1
        elsif c == "%" && i + 2 < n
          hi = _hex_nibble(s[i + 1])
          lo = _hex_nibble(s[i + 2])
          if hi >= 0 && lo >= 0
            out = out + ((hi * 16 + lo).chr)
            i += 3
          else
            out = out + c
            i += 1
          end
        else
          out = out + c
          i += 1
        end
      end
      out
    end

    def self._hex_char(n)
      return ("0".bytes[0] + n).chr if n < 10
      ("a".bytes[0] + n - 10).chr
    end

    def self._hex_nibble(c)
      return c.bytes[0] - "0".bytes[0] if c >= "0" && c <= "9"
      return c.bytes[0] - "a".bytes[0] + 10 if c >= "a" && c <= "f"
      return c.bytes[0] - "A".bytes[0] + 10 if c >= "A" && c <= "F"
      -1
    end

    def self._parse_query(s)
      out = {}
      return out if s.length == 0
      s.split("&").each do |pair|
        eq = pair.index("=")
        next if eq.nil? || eq < 0
        k = _url_unescape(pair[0, eq])
        v = _url_unescape(pair[(eq + 1)..])
        out[k] = v
      end
      out
    end
  end
end
