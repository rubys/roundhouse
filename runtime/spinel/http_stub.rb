# HttpStub — the table that `WebMock.stub_request(...).to_return(...)`
# lowers to (`lower::webmock`), and the seam `runtime/net_http.rb`'s
# reopened `Net::HTTP#request` consults before it touches a socket.
#
# THE RUBY FAMILY NEVER LOADS THIS FILE. `project::ruby_runtime_files`
# swaps it for a three-method delegate onto the real WebMock gem, the
# way it swaps `resolv.rb` for the stdlib resolver: the lowering is
# uniform across targets (a lowering may not branch on the target), so
# the FILE is what differs. Over there a stub lands in WebMock's own
# registry and intercepts CRuby's `Net::HTTP`; over here it lands in
# these arrays and the reopen answers from them.
#
# WHAT A STUB IS. campfire's forty `stub_request` sites match on
# nothing but `(verb, url)` — no `.with(...)` request matcher reaches
# this table, because a chain the lowering does not fully understand
# keeps its WebMock spelling and fails loudly at the `WebMock` constant.
# So the key is the verb and the normalised URL, and the value is the
# three things `to_return` carries: a status, a body, and headers.
#
# Parallel constant Arrays, not a Hash and not a module-level ivar —
# the idiom `runtime/broadcasts.rb` establishes and `resolv.rb` follows:
# spinel supports constants and array mutation; module-level instance
# variables are less certain. SEEDED for the reason `Broadcasts::TRANSPORTS`
# gives: an always-empty literal leaves spinel nothing to infer an
# element type from, and the whole table lands behind unresolved-call
# gates. The seeds can never match — no request has an empty verb.
require "uri"

require "json"

module HttpStub
  STUB_VERBS = [ "" ]
  STUB_URLS = [ "" ]
  STUB_STATUSES = [ 0 ]
  STUB_BODIES = [ "" ]
  # Headers as ONE String per stub — `name:value\n` lines, the shape
  # `read_response` parses off the wire — and not as a Hash. An
  # `Array[Hash[String, String]]` element read anywhere in the program
  # cost spinel's analysis of every campfire test binary ~4 minutes
  # (23s -> 241s on user_bot_test, measured 2026-09-10, whichever way the
  # element was read: passed, bound to a local, or iterated). Four
  # parallel String arrays are what the table is; this keeps it so.
  STUB_HEADER_LINES = [ "" ]
  # A request-body matcher per stub — `.with(body: hash_including(h))`
  # — as the JSON text of `h`, or `""` for a stub that matches any body.
  # Text for the same reason the headers are: one more String column
  # keeps the table's element types flat. Both sides are parsed back to
  # objects at match time (`subset?`).
  STUB_MATCHER_JSON = [ "" ]

  # One spelling for a URL, whichever side wrote it. WebMock treats
  # `https://www.example.com/` and `https://www.example.com:443/` as the
  # same stub, and campfire's fetch tests stub the former while
  # `URI.parse("https://www.example.com")` — no trailing slash — is what
  # the app requests; `request_uri` answers `/` for both. The request
  # side rebuilds the same shape from the connection's address, port and
  # the request path (`Net::HTTP#stub_url`).
  def self.normalize(url)
    u = URI.parse(url)
    scheme = u.scheme.to_s.downcase
    host = u.host.to_s.downcase
    port = u.port.to_i
    default = scheme == "https" ? 443 : 80
    authority = port == default || port == 0 ? host : "#{host}:#{port}"
    "#{scheme}://#{authority}#{u.request_uri}"
  end

  # Install or REPLACE one `(verb, url)` answer. Replacement matters:
  # campfire's fetch tests re-stub the same URL within one test (a 302
  # to a host, then that host's 200), and a write-once slot would
  # answer the first value twice.
  #
  # `headers` arrives with lower-cased, dash-spelled names and String
  # values — `lower::webmock` normalises both spellings campfire writes
  # (`content_type:` and `"Content-Type" =>`) on the way in — which is
  # the shape spinel's `Net::HTTPResponse` stores, so `response["Content-Type"]`
  # and `response.content_type` read them without a second pass.
  def self.stub(verb, url, status, body, headers)
    file(verb, url, status, body, headers, "")
  end

  # `WebMock.stub_request(verb, url).with(body: hash_including(expected))`
  # — answers only a request whose JSON body carries every key of
  # `expected` with an equal value, nested Hashes recursively (WebMock's
  # `hash_including`). `expected_json` is the test's literal, whatever
  # it nests, generated to text AT THE CALL SITE by `lower::webmock`: a
  # typed nested Hash handed through an untyped parameter reached
  # `JSON.generate` as a boxed value it could not read (a crash in
  # `sp_str_byte_len`), where the same literal generated in place is
  # fine. Text is also what the column holds (`STUB_MATCHER_JSON`).
  def self.stub_matching(verb, url, status, body, headers, expected_json)
    file(verb, url, status, body, headers, expected_json)
  end

  def self.file(verb, url, status, body, headers, matcher_json)
    v = verb.to_s.upcase
    u = normalize(url)
    lines = header_lines(headers)
    i = 0
    while i < STUB_URLS.length
      if STUB_VERBS[i] == v && STUB_URLS[i] == u && STUB_MATCHER_JSON[i] == matcher_json
        STUB_STATUSES[i] = status
        STUB_BODIES[i] = body
        STUB_HEADER_LINES[i] = lines
        return nil
      end
      i += 1
    end
    STUB_VERBS << v
    STUB_URLS << u
    STUB_STATUSES << status
    STUB_BODIES << body
    STUB_HEADER_LINES << lines
    STUB_MATCHER_JSON << matcher_json
    nil
  end

  # `{ "content-type" => "text/html" }` -> `"content-type:text/html\n"`.
  # A header value never carries a newline (the wire forbids it), so the
  # split on the other side is exact.
  def self.header_lines(headers)
    out = String.new
    headers.each { |k, v| out << k.to_s.downcase << ":" << v.to_s << "\n" }
    out
  end

  # The inverse, for the reopen: a FRESH Hash, which is what the
  # response wants to own.
  def self.headers_at(i)
    h = {}
    STUB_HEADER_LINES[i].split("\n").each do |line|
      ci = line.index(":")
      h[line[0, ci]] = line[(ci + 1)..-1].to_s unless ci.nil?
    end
    h
  end

  # The index of the stub answering `(verb, url)`, or -1. The reopen
  # reads the three value arrays at that index.
  def self.find(verb, url)
    find_for(verb, url, "")
  end

  # The same, for a request carrying `body`: a stub with a matcher
  # answers only when the body satisfies it. A stub for the URL that
  # matches on body and does not match THIS body RAISES, which is what
  # WebMock does for a request no stub accepts — the test asserted the
  # body's shape, and a silent pass through to the network is the
  # failure that matters.
  def self.find_for(verb, url, body)
    v = verb.to_s.upcase
    i = 0
    saw_matcher = false
    while i < STUB_URLS.length
      if STUB_VERBS[i] == v && STUB_URLS[i] == url
        return i if STUB_MATCHER_JSON[i] == ""
        saw_matcher = true
        return i if body_matches?(body, STUB_MATCHER_JSON[i])
      end
      i += 1
    end
    raise "WebMock: #{v} #{url} was stubbed with a body matcher the request body did not satisfy: #{body}" if saw_matcher
    -1
  end

  def self.body_matches?(body, matcher_json)
    return false if body.nil? || body.to_s == ""
    subset?(JSON.parse(body.to_s), JSON.parse(matcher_json))
  end

  # WebMock's `hash_including`, over two parsed JSON values: every key
  # of `expected` is present in `actual` with an equal value, a nested
  # Hash recursively. Keys compare as text — the expectation was
  # written with Symbols and the wire carries Strings.
  def self.subset?(actual, expected)
    return false if !actual.is_a?(Hash) || !expected.is_a?(Hash)
    ok = true
    expected.each do |k, v|
      key = k.to_s
      if !actual.key?(key)
        ok = false
      elsif v.is_a?(Hash)
        ok = false if !subset?(actual[key], v)
      elsif actual[key] != v
        ok = false
      end
    end
    ok
  end

  # `WebMock.disable_net_connect!(allow: [...])`. There is no transport
  # to allow or deny here: a request with no stub goes to the real
  # `Net::HTTP` exactly as it would with no double installed. Accepted
  # so the lowering stays uniform; the ruby family's delegate passes it
  # on to WebMock, where it means something.
  def self.allow_net_connect(hosts)
    nil
  end

  # Drop every installed stub. The emitted helper's setup calls this,
  # so a stub cannot outlive the test that wrote it — the same leak
  # WebMock's own `after_teardown` exists to prevent, and the one that
  # took `opengraph_location_test` from 7/7 to 6/7 when the resolv
  # slot shipped without its clear.
  def self.clear
    STUB_VERBS.clear
    STUB_URLS.clear
    STUB_STATUSES.clear
    STUB_BODIES.clear
    STUB_HEADER_LINES.clear
    STUB_MATCHER_JSON.clear
    STUB_VERBS << ""
    STUB_URLS << ""
    STUB_STATUSES << 0
    STUB_BODIES << ""
    STUB_HEADER_LINES << ""
    STUB_MATCHER_JSON << ""
    nil
  end
end
