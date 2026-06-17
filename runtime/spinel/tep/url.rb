# Percent-decoding + form-urlencoded query parser.
module Tep
  class Url
    # "%41+b" -> "A b"
    def self.unescape(s)
      out = ""
      i = 0
      n = s.length
      while i < n
        c = s[i]
        if c == "+"
          out << " "
          i += 1
        elsif c == "%" && i + 2 < n
          hi = Url.hex_nibble(s[i + 1])
          lo = Url.hex_nibble(s[i + 2])
          if hi >= 0 && lo >= 0
            out << ((hi * 16 + lo).chr)
            i += 3
          else
            out << c
            i += 1
          end
        else
          out << c
          i += 1
        end
      end
      out
    end

    # Percent-encode the bytes that are unsafe in cookie values, query
    # strings, and similar contexts. RFC 3986 unreserved set:
    # ALPHA / DIGIT / `-._~`. Everything else gets `%XX` (uppercase hex).
    def self.escape(s)
      out = ""
      i = 0
      while i < s.length
        c = s[i]
        if (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") ||
           (c >= "0" && c <= "9") || c == "-" || c == "." ||
           c == "_" || c == "~"
          out << c
        else
          b = c.bytes[0]
          hi = b / 16
          lo = b % 16
          out << "%" + Url.hex_char(hi) + Url.hex_char(lo)
        end
        i += 1
      end
      out
    end

    def self.hex_char(n)
      if n < 10
        return ("0".bytes[0] + n).chr
      end
      ("A".bytes[0] + n - 10).chr
    end

    def self.hex_nibble(c)
      if c >= "0" && c <= "9"
        return c.bytes[0] - "0".bytes[0]
      end
      if c >= "a" && c <= "f"
        return c.bytes[0] - "a".bytes[0] + 10
      end
      if c >= "A" && c <= "F"
        return c.bytes[0] - "A".bytes[0] + 10
      end
      -1
    end

    # Split a URL into a Hash with str=>str entries:
    #   "scheme" "host" "port" "path" "query"
    #
    # Recognises `http://host[:port]/path?query` and the same shape
    # with `https://`. Without a scheme, the input is treated as a
    # path (host stays empty); useful for routing relative paths
    # through the same parser. Default ports follow the scheme:
    # 80 for http, 443 for https. Path defaults to "/". `query` is
    # the raw substring after `?`, no further decoding.
    #
    # Inlined as one method on purpose: spinel's analyzer widens
    # Hash-typed parameters when a helper mutates them and the
    # caller then keeps reading; sticking to a single body keeps
    # `out` narrowed to StrStrHash throughout.
    def self.split_url(u)
      out = Tep.str_hash
      out["scheme"] = ""
      out["host"]   = ""
      out["port"]   = ""
      out["path"]   = "/"
      out["query"]  = ""

      rest = u
      if rest.length >= 7 && rest[0, 7] == "http://"
        out["scheme"] = "http"
        out["port"]   = "80"
        rest = rest[7, rest.length - 7]
      elsif rest.length >= 8 && rest[0, 8] == "https://"
        out["scheme"] = "https"
        out["port"]   = "443"
        rest = rest[8, rest.length - 8]
      end

      if out["scheme"].length > 0
        slash = rest.index("/")
        hostport = rest
        tail     = "/"
        unless slash.nil?
          hostport = rest[0, slash]
          tail     = rest[slash, rest.length - slash]
        end
        colon = hostport.index(":")
        if colon.nil?
          out["host"] = hostport
        else
          out["host"] = hostport[0, colon]
          out["port"] = hostport[colon + 1, hostport.length - colon - 1]
        end
        rest = tail
      end

      qi = rest.index("?")
      if qi.nil?
        out["path"] = rest
      else
        out["path"]  = rest[0, qi]
        out["query"] = rest[qi + 1, rest.length - qi - 1]
      end
      if out["path"].length == 0
        out["path"] = "/"
      end
      out
    end

    # "a=1&b=2&c" -> Hash {"a"=>"1","b"=>"2","c"=>""}
    def self.parse_query(s)
      h = Tep.str_hash
      if s.length == 0
        return h
      end
      pairs = s.split("&")
      pairs.each do |pair|
        if pair.length > 0
          eq = pair.index("=")
          if eq.nil?
            h[Url.unescape(pair)] = ""
          else
            k = pair[0, eq]
            v = pair[eq + 1, pair.length - eq - 1]
            h[Url.unescape(k)] = Url.unescape(v)
          end
        end
      end
      h
    end
  end
end
