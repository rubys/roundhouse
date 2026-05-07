# CGI protocol I/O. Reads requests from ENV + stdin; writes responses
# to stdout. The shape spinel can ingest (no sockets) and the shape
# any CGI-aware web server (Apache mod_cgi, nginx fcgiwrap, lighttpd)
# can drive.
#
# Inputs (per the CGI/1.1 spec):
#   ENV["REQUEST_METHOD"]   — "GET" / "POST" / "PATCH" / "DELETE"
#   ENV["PATH_INFO"]        — path portion, e.g. "/articles/42"
#   ENV["QUERY_STRING"]     — raw query string, e.g. "foo=bar"
#   ENV["CONTENT_LENGTH"]   — body length (decimal string)
#   ENV["CONTENT_TYPE"]     — "application/x-www-form-urlencoded" supported
#   stdin                   — the request body (for POST/PATCH/PUT)
#
# Outputs (the response is plain text on stdout):
#   Status: <code> <reason>\r\n
#   Content-Type: text/html; charset=utf-8\r\n
#   [Location: <url>\r\n  if redirect]
#   \r\n
#   <body bytes>
#
# Pure Ruby; no `cgi` stdlib dependency (which spinel doesn't ship).
# Keeps the spinel-subset envelope clean: basic regex, string ops,
# Hash mutation. No metaprogramming.
module CgiIo
  REASON_PHRASES = {
    200 => "OK",
    201 => "Created",
    204 => "No Content",
    301 => "Moved Permanently",
    302 => "Found",
    303 => "See Other",
    304 => "Not Modified",
    400 => "Bad Request",
    401 => "Unauthorized",
    403 => "Forbidden",
    404 => "Not Found",
    422 => "Unprocessable Entity",
    500 => "Internal Server Error",
  }.freeze

  # Parse a CGI request from the given env hash + body-readable IO.
  # Returns: { method:, path:, params:, cookies: }.
  def self.parse_request(env, stdin)
    method = (env["REQUEST_METHOD"] || "GET").upcase
    path   = env["PATH_INFO"] || "/"
    query  = env["QUERY_STRING"] || ""

    params = {}
    parse_form_into(query, params) unless query.empty?

    if method == "POST" || method == "PATCH" || method == "PUT"
      length = (env["CONTENT_LENGTH"] || "0").to_i
      ctype  = env["CONTENT_TYPE"] || ""
      if length > 0 && ctype.start_with?("application/x-www-form-urlencoded")
        body = stdin.read(length).to_s
        parse_form_into(body, params)
      end
    end

    # Rails-style method override: a POST with hidden `_method=delete` (or
    # patch / put) is treated as that verb for routing purposes. Browsers
    # can't emit DELETE/PATCH directly from a form, so Rails renders these
    # forms as method="post" with a hidden input; the dispatch layer
    # rewrites the verb here. Only honored from POST → {PATCH,PUT,DELETE};
    # GETs ignore the override.
    if method == "POST"
      override = params.delete(:_method)
      if !override.nil?
        upcased = override.to_s.upcase
        method = upcased if upcased == "PATCH" || upcased == "PUT" || upcased == "DELETE"
      end
    end

    cookies = parse_cookies(env["HTTP_COOKIE"])

    { method: method, path: path, params: params, cookies: cookies }
  end

  # Write a CGI response to the given writable IO. `set_cookies` is
  # `{ name => value | nil }`; nil clears the cookie via Max-Age=0.
  def self.write_response(io, status, body, location: nil, content_type: "text/html; charset=utf-8", set_cookies: {})
    code   = status.is_a?(Integer) ? status : status.to_i
    reason = REASON_PHRASES.fetch(code, "OK")
    io.write("Status: #{code} #{reason}\r\n")
    io.write("Content-Type: #{content_type}\r\n")
    io.write("Location: #{location}\r\n") unless location.nil?
    set_cookies.each do |name, val|
      if val.nil?
        io.write("Set-Cookie: #{name}=; Path=/; Max-Age=0\r\n")
      else
        io.write("Set-Cookie: #{name}=#{url_encode(val.to_s)}; Path=/; HttpOnly\r\n")
      end
    end
    io.write("\r\n")
    io.write(body.to_s)
    nil
  end

  # ── cookie parsing ──────────────────────────────────────────────

  # Parse a `Cookie:` header value into a Hash[Symbol => String].
  # The `; ` separator between cookies is RFC 6265 standard; tolerate
  # extra whitespace around the `=` and around separators.
  def self.parse_cookies(header)
    out = {}
    return out if header.nil? || header.empty?
    header.split(";").each do |pair|
      pair = pair.strip
      eq = pair.index("=")
      next if eq.nil?
      name = url_decode(pair[0, eq].strip)
      val  = url_decode(pair[(eq + 1)..].to_s.strip)
      out[name.to_sym] = val unless name.empty?
    end
    out
  end

  # ── form-urlencoded parsing ─────────────────────────────────────

  # Parse a `key1=val1&key2=val2&article[title]=hello` body into a
  # nested-hash structure. Mutates the passed-in hash so multiple
  # sources (query string + body) can be merged.
  def self.parse_form_into(input, into)
    return if input.empty?
    input.split("&").each do |pair|
      next if pair.empty?
      eq = pair.index("=")
      raw_key = eq.nil? ? pair : pair[0, eq]
      raw_val = eq.nil? ? ""   : pair[(eq + 1)..]
      key = url_decode(raw_key)
      val = url_decode(raw_val)
      assign_form_pair(into, key, val)
    end
    nil
  end

  # `article[title]` → into[:article][:title] = val
  # `id` → into[:id] = val
  def self.assign_form_pair(into, raw_key, val)
    open_bracket = raw_key.index("[")
    if open_bracket.nil?
      into[raw_key.to_sym] = val
      return
    end
    close_bracket = raw_key.index("]", open_bracket + 1)
    return if close_bracket.nil?
    outer = raw_key[0, open_bracket].to_sym
    inner = raw_key[(open_bracket + 1)...close_bracket].to_sym
    into[outer] = {} unless into[outer].is_a?(Hash)
    into[outer][inner] = val
  end

  # Spinel-friendly URL encode: pass-through for unreserved chars
  # (RFC 3986 §2.3: ALPHA / DIGIT / `-` / `.` / `_` / `~`); percent-
  # encode everything else as `%XX`. Used for cookie values; cookies
  # are Latin-1 in the wire format but our values are ASCII for the
  # demo, so byte-by-byte is sufficient.
  def self.url_encode(s)
    out = String.new
    s.to_s.each_byte do |b|
      if (b >= 48 && b <= 57) || (b >= 65 && b <= 90) || (b >= 97 && b <= 122) ||
         b == 45 || b == 46 || b == 95 || b == 126
        out << b.chr
      else
        out << format("%%%02X", b)
      end
    end
    out
  end

  # Spinel-friendly URL decode: % escapes + `+` → space.
  #
  # CRuby caveat: Integer#chr returns an ASCII-8BIT-encoded single-byte
  # String, which propagates through concatenation and lands in the DB
  # as a BLOB rather than TEXT — breaking subsequent `WHERE col = ?`
  # comparisons against UTF-8 literals. Force the result back to UTF-8.
  # Spinel itself "assumes UTF-8/ASCII" per its README, so this
  # encoding dance is a CRuby-only concern.
  def self.url_decode(s)
    out = String.new
    i = 0
    n = s.length
    while i < n
      ch = s[i]
      if ch == "+"
        out << " "
        i += 1
      elsif ch == "%" && i + 2 < n
        hex = s[(i + 1), 2]
        if hex =~ /\A[0-9A-Fa-f]{2}\z/
          out << hex.to_i(16).chr
          i += 3
        else
          out << ch
          i += 1
        end
      else
        out << ch
        i += 1
      end
    end
    out.force_encoding("UTF-8")
  end
end
