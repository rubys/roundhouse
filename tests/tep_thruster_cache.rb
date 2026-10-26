# Driver for tests/tep_thruster_cache.rs — Tep::ThrusterCache, the spinel
# binary's in-process Thruster cache, held to the rules its header claims
# (the same list tests/thruster_cache.rs holds the CRuby overlay to).
#
# Run under plain CRuby through the real threaded server over the
# scripted socket (tests/tep_server_harness.rb), so each check reads what
# went out on the wire. The app consults the cache as Tep::App#dispatch
# does: `answer` before the app, `store` after it.

require_relative "tep_server_harness"
require_relative "../runtime/spinel/tep/thruster_cache"

$clock = 1000.0
Tep::ThrusterCache.define_singleton_method(:now) { $clock }

RESPONSES = {
  "/public" => [200, { "Content-Type" => "text/css", "Cache-Control" => "public, max-age=60",
                       "ETag" => "\"v1\"" }, "body-public"],
  "/private" => [200, { "Cache-Control" => "max-age=0, private, must-revalidate" }, "body-private"],
  "/nocache" => [200, { "Cache-Control" => "public, no-cache, max-age=60" }, "n"],
  "/zero" => [200, { "Cache-Control" => "public, max-age=0" }, "z"],
  "/star" => [200, { "Cache-Control" => "public, max-age=60", "Vary" => "*" }, "s"],
  "/notfound" => [404, { "Cache-Control" => "public, max-age=60" }, "nf"],
  "/smax" => [200, { "Cache-Control" => "public, s-max-age=60, max-age=0" }, "sm"],
  "/vary" => [200, { "Cache-Control" => "public, max-age=60", "Vary" => "Accept" }, nil],
  "/big" => [200, { "Cache-Control" => "public, max-age=60" }, "x" * ((1 << 20) + 1)],
}

class RecordingApp
  attr_reader :calls

  def dispatch(req, res)
    return true if Tep::THRUSTER_CACHE.answer(req, res)
    @calls ||= Hash.new(0)
    @calls[req.path] += 1
    status, headers, body = RESPONSES.fetch(req.path)
    res.status = status
    headers.each { |k, v| res.headers[k] = v }
    res.set_cookie("s", "1", {})
    res.body = body || "accept=#{req.req_headers.fetch("accept", "")}"
    Tep::THRUSTER_CACHE.store(req, res)
    true
  end
end

def get(path, extra = "", verb: "GET")
  bytes = "#{verb} #{path} HTTP/1.1\r\nHost: localhost\r\n#{extra}\r\n".b
  status, = serve("threaded", bytes)
  head, body = Sock.wire.out.split("\r\n\r\n", 2)
  lines = head.to_s.split("\r\n")
  [status, lines, body.to_s]
end

def header(lines, name)
  line = lines.find { |l| l.downcase.start_with?("#{name.downcase}:") }
  line && line.split(": ", 2)[1]
end

def calls(path) = APP.calls[path]

# stored once, answered from memory after; Set-Cookie gone from both
s, h, b = get("/public")
check("first public GET is a miss", s == 200 && header(h, "x-cache") == "miss" && b == "body-public", h.inspect)
check("a stored response's miss loses Set-Cookie", header(h, "set-cookie").nil?, h.inspect)
check("a stored response without Vary gains Vary: Accept-Encoding", header(h, "vary") == "Accept-Encoding", h.inspect)
s, h, b = get("/public")
check("second public GET is a hit", calls("/public") == 1 && header(h, "x-cache") == "hit" && b == "body-public", h.inspect)
check("a hit carries no Set-Cookie", header(h, "set-cookie").nil?, h.inspect)

# 304 for a matching If-None-Match; HEAD has its own key and no body
s, h, b = get("/public", "If-None-Match: \"v0\", \"v1\"\r\n")
check("a hit answers 304 to its ETag", s == 304 && b.empty? && header(h, "content-type").nil?, [s, h, b].inspect)
get("/public", verb: "HEAD")
s, h, b = get("/public", verb: "HEAD")
check("a HEAD hit has no body", s == 200 && header(h, "x-cache") == "hit" && b.empty? &&
      header(h, "content-length") == "11", [s, h, b].inspect)

# expiry
$clock += 61
_, h, = get("/public")
check("an expired entry is fetched again", header(h, "x-cache") == "miss" && calls("/public") == 3, h.inspect)

# s-max-age wins over max-age
2.times { get("/smax") }
check("s-max-age is the lifetime when present", calls("/smax") == 1)

# never stored
%w[/private /nocache /zero /star /notfound /big].each do |path|
  2.times { get(path) }
  check("#{path} is not stored", calls(path) == 2, calls(path).inspect)
end
_, h, b = get("/private")
check("a response kept out of the store is a miss that keeps its Set-Cookie",
      header(h, "x-cache") == "miss" && header(h, "set-cookie") == "s=1", h.inspect)
_, _, b = get("/big")
check("an oversized body is still served whole", b.bytesize == (1 << 20) + 1)

# bypass: Range, upgrade, long URIs, disabled
before = calls("/public")
_, h, = get("/public", "Range: bytes=0-1\r\n")
check("Range bypasses and says so", header(h, "x-cache") == "bypass", h.inspect)
get("/public", "Upgrade: websocket\r\n")
get("/public?" + "q" * 2048)
check("Range, upgrade and a long URI reach the app", calls("/public") == before + 3, calls("/public").inspect)
Tep::THRUSTER_CACHE.enabled = false
get("/public")
check("ROUNDHOUSE_RESPONSE_CACHE=0 turns it off", calls("/public") == before + 4)
Tep::THRUSTER_CACHE.enabled = true

# the query string is part of the key
get("/public?a=1")
get("/public?a=2")
get("/public?a=1")
check("each query string is its own entry", calls("/public") == before + 6, calls("/public").inspect)

# Vary: keyed by the request's value for each named header
a1 = get("/vary", "Accept: text/a\r\n")[2]
b1 = get("/vary", "Accept: text/b\r\n")[2]
a2 = get("/vary", "Accept: text/a\r\n")[2]
b2 = get("/vary", "Accept: text/b\r\n")[2]
check("variants are not crossed", a1 == "accept=text/a" && b1 == "accept=text/b", [a1, b1].inspect)
check("each variant is stored once", a2 == a1 && b2 == b1 && calls("/vary") == 2, [a2, b2, calls("/vary")].inspect)

# capacity: least recently used evicted first
small = Tep::ThrusterCache.new(2000) # three ~630-byte entries fit, four do not
Tep.send(:remove_const, :THRUSTER_CACHE)
Tep.const_set(:THRUSTER_CACHE, small)
4.times { |i| RESPONSES["/lru#{i}"] = [200, { "Cache-Control" => "public, max-age=60" }, "y" * 300] }
%w[/lru0 /lru1 /lru2 /lru0 /lru3].each { |p| get(p) }
get("/lru0")
get("/lru1")
check("the least recently used entry is evicted", calls("/lru0") == 1 && calls("/lru1") == 2,
      [calls("/lru0"), calls("/lru1")].inspect)
check("the store stays within its capacity", small.size <= small.capacity, small.size.inspect)

total = CHECKS.length
passed = CHECKS.count(true)
puts "#{passed}/#{total} checks pass"
puts "done"
