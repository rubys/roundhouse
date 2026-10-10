# HTTP/1.x request parser. Produces a Tep::Request from the raw
# byte blob the C helper read off the wire (headers, possibly a
# prefix of the body).
module Tep
  # Owned by one connection, never by an fd number or the server singleton.
  # Headers can arrive with bytes belonging to a later pipelined request.
  class InputBuffer
    def initialize
      @pending_input = +"".b
    end

    def take_pending_input
      pending = @pending_input
      @pending_input = +"".b
      pending
    end

    def keep_pending_input(bytes)
      @pending_input = bytes
      nil
    end
  end

  class Parser
    # Returns a fully-populated Request, or nil if the blob is malformed.
    def self.parse(blob, input = InputBuffer.new)
      # Delimit headers and bodies as octets, even with a UTF-8-tagged recv.
      blob = blob.b
      # String#index returns nil (not -1) when not found — matches CRuby.
      # See matz/spinel#532; spinel 0210389 fixed the prior -1 sentinel.
      end_of_headers = blob.index("\r\n\r\n")
      if end_of_headers.nil?
        return nil
      end
      headers_blob = blob[0, end_of_headers]
      lines = headers_blob.split("\r\n")
      if lines.length == 0
        return nil
      end

      first = lines[0]
      first_parts = first.split(" ")
      if first_parts.length < 3
        return nil
      end

      req = Request.new
      req.verb         = first_parts[0]
      req.raw_path     = first_parts[1]
      req.http_version = first_parts[2]

      qmark = req.raw_path.index("?")
      if qmark.nil?
        req.path = req.raw_path
      else
        req.path  = req.raw_path[0, qmark]
        qstring   = req.raw_path[qmark + 1, req.raw_path.length - qmark - 1]
        req.query = Url.parse_query(qstring)
        req.raw_query = qstring
      end

      i = 1
      while i < lines.length
        line = lines[i]
        # Reject malformed field names and obs-fold before reading framing.
        if line.gsub(/\A[!#$%&'*+.^_`|~0-9A-Za-z-]+:/, "") == line
          return nil
        end
        colon = line.index(":")
        unless colon.nil?
          name  = line[0, colon].downcase
          value = line[colon + 1, line.length - colon - 1]
          if name == "content-length"
            # Only SP and HTAB are field whitespace. String#strip would
            # hide an invalid trailing NUL, CR, LF, VT or FF.
            value = value.gsub(/\A[ \t]+|[ \t]+\z/, "")
            # Validate before assignment: a later field must not hide an
            # invalid value or a different length (RFC 9112, 6.3 rule 5).
            if value.length == 0 || Tep.decimal_byte_count(value) < 0
              return nil
            end
            if req.req_headers.key?(name) && req.req_headers[name] != value
              return nil
            end
          else
            value = value.strip
          end
          req.req_headers[name] = value
        end
        i += 1
      end

      # Pre-merge query into params; path captures will be folded in
      # by the router on a successful match.
      req.query.each do |k, v|
        req.req_params[k] = v
      end

      # Parse Cookie header into req.cookies. Format: "k=v; k2=v2; ...".
      # Whitespace around `;` is allowed and stripped.
      cookie_blob = req.req_headers["cookie"]
      if cookie_blob.length > 0
        cookie_blob.split(";").each do |pair|
          eq = pair.index("=")
          if !eq.nil? && eq > 0
            cname  = pair[0, eq].strip
            cvalue = pair[eq + 1, pair.length - eq - 1].strip
            req.cookies[cname] = Url.unescape(cvalue)
          end
        end
      end

      # Content-Length ends this body exactly, including zero (or an
      # absent header). Keep surplus bytes for this connection's next
      # request instead of appending them to the body or dropping them.
      body_start = end_of_headers + 4
      received = blob.bytesize - body_start
      body_bytes = req.content_length
      if body_bytes < 0
        body_bytes = 0   # body_refusal will reject the invalid length
      end
      if body_bytes > received
        body_bytes = received
      end
      req.raw_body = blob.byteslice(body_start, body_bytes)
      input.keep_pending_input(blob.byteslice(body_start + body_bytes, received - body_bytes))

      req
    end
  end
end
