//! ThrusterCache: Thruster's response cache, in process, for the CRuby
//! overlay. Each rule the module's header claims, held to it.

use std::process::Command;

const SCRIPT: &str = r##"
require "rack"
require File.expand_path("runtime/spinel/scaffold/ruby_overlay/runtime/thruster_cache", Dir.pwd)

$clock = 1000.0
Process.singleton_class.prepend(Module.new do
  def clock_gettime(id, *rest)
    id == Process::CLOCK_MONOTONIC ? $clock : super
  end
end)

calls = Hash.new(0)
responses = {
  "/public" => [200, { "content-type" => "text/css", "cache-control" => "public, max-age=60",
                       "set-cookie" => "s=1", "etag" => "\"v1\"" }, ["body-public"]],
  "/private" => [200, { "cache-control" => "max-age=0, private, must-revalidate" }, ["body-private"]],
  "/nocache" => [200, { "cache-control" => "public, no-cache, max-age=60" }, ["n"]],
  "/zero" => [200, { "cache-control" => "public, max-age=0" }, ["z"]],
  "/star" => [200, { "cache-control" => "public, max-age=60", "vary" => "*" }, ["s"]],
  "/notfound" => [404, { "cache-control" => "public, max-age=60" }, ["nf"]],
  "/vary" => [200, { "cache-control" => "public, max-age=60", "vary" => "Accept" }, nil],
  "/big" => [200, { "cache-control" => "public, max-age=60" }, ["x" * ((1 << 20) + 1)]],
  "/hijack" => [-1, {}, []],
}
app = lambda do |env|
  path = env["PATH_INFO"]
  calls[path] += 1
  status, headers, body = responses.fetch(path)
  body ||= ["accept=#{env["HTTP_ACCEPT"]}"]
  [status, headers.dup, body.dup]
end
mw = ThrusterCache::Middleware.new(app)
get = ->(path, extra = {}) { mw.call(Rack::MockRequest.env_for(path, extra)) }
check = ->(cond, what) { raise "FAILED: #{what}" unless cond }

# stored once, answered from memory after; Set-Cookie gone from both
s, h, b = get.("/public")
check.(s == 200 && h["x-cache"] == "miss" && b.join == "body-public", "first public GET is a miss")
check.(!h.key?("set-cookie"), "a stored response's miss loses Set-Cookie")
check.(h["vary"] == "Accept-Encoding", "a stored response without Vary gains Vary: Accept-Encoding")
s, h, b = get.("/public")
check.(calls["/public"] == 1 && h["x-cache"] == "hit" && b.join == "body-public", "second public GET is a hit")
check.(!h.key?("set-cookie"), "a hit carries no Set-Cookie")

# 304 for a matching If-None-Match; HEAD has its own key and no body
s, h, b = get.("/public", "HTTP_IF_NONE_MATCH" => "\"v0\", \"v1\"")
check.(s == 304 && b.empty? && !h.key?("content-type"), "a hit answers 304 to its ETag")
get.("/public", method: "HEAD")
s, h, b = get.("/public", method: "HEAD")
check.(s == 200 && h["x-cache"] == "hit" && b.empty?, "HEAD hit has no body")

# expiry
$clock += 61
s, h, = get.("/public")
check.(h["x-cache"] == "miss" && calls["/public"] == 3, "an expired entry is fetched again")

# never stored
%w[/private /nocache /zero /star /notfound /big].each do |path|
  2.times { get.(path) }
  check.(calls[path] == 2, "#{path} is not stored")
end
s, h, b = get.("/big")
check.(b.join.bytesize == (1 << 20) + 1, "an oversized body is still served whole")

# bypass: methods, Range, upgrade, long URIs, disabled
s, h, = mw.call(Rack::MockRequest.env_for("/public", method: "POST"))
check.(h["x-cache"] == "bypass", "POST bypasses")
before = calls["/public"]
get.("/public", "HTTP_RANGE" => "bytes=0-1")
get.("/public", "HTTP_UPGRADE" => "websocket")
get.("/public?" + "q" * 2048)
check.(calls["/public"] == before + 3, "Range, upgrade and a long URI bypass")
ENV["ROUNDHOUSE_RESPONSE_CACHE"] = "0"
get.("/public")
check.(calls["/public"] == before + 4, "ROUNDHOUSE_RESPONSE_CACHE=0 turns it off")
ENV.delete("ROUNDHOUSE_RESPONSE_CACHE")

# Vary: keyed by the request's value for each named header
a1 = get.("/vary", "HTTP_ACCEPT" => "text/a")[2].join
b1 = get.("/vary", "HTTP_ACCEPT" => "text/b")[2].join
a2 = get.("/vary", "HTTP_ACCEPT" => "text/a")[2].join
b2 = get.("/vary", "HTTP_ACCEPT" => "text/b")[2].join
check.(a1 == "accept=text/a" && b1 == "accept=text/b", "variants are not crossed")
check.(a2 == a1 && b2 == b1 && calls["/vary"] == 2, "each variant is stored once")

# a hijacked response passes through untouched
s, h, = get.("/hijack")
check.(s == -1 && h.empty?, "a hijack tuple is left alone")

# capacity: least recently used evicted first
ThrusterCache.instance_variable_set(:@items, {})
ThrusterCache.instance_variable_set(:@size, 0)
ThrusterCache.send(:remove_const, :CAPACITY)
ThrusterCache.const_set(:CAPACITY, 2000) # three ~630-byte entries fit, four do not
3.times { |i| responses["/lru#{i}"] = [200, { "cache-control" => "public, max-age=60" }, ["y" * 300]] }
responses["/lru3"] = [200, { "cache-control" => "public, max-age=60" }, ["y" * 300]]
%w[/lru0 /lru1 /lru2 /lru0 /lru3].each { |p| get.(p) }
get.("/lru0"); get.("/lru1")
check.(calls["/lru0"] == 1 && calls["/lru1"] == 2, "the least recently used entry is evicted")

puts "ALL OK"
"##;

#[test]
fn thruster_cache_follows_thrusters_rules() {
    let output = Command::new("ruby")
        .args(["-e", SCRIPT])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Ruby interpreter available");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "thruster cache failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("ALL OK"), "thruster cache cases did not execute:\n{stdout}");
}

// The Falcon front answers from the store the Rack middleware fills, as
// a Protocol::HTTP response, and hands everything else to the app behind
// it. Protocol::HTTP is stood in for by the three shapes Front builds, so
// the rules are held here without Falcon's gems; the real classes are
// exercised by the ruby-falcon bench lane.
const FRONT_SCRIPT: &str = r##"
require "rack"
require File.expand_path("runtime/spinel/scaffold/ruby_overlay/runtime/thruster_cache", Dir.pwd)

module Protocol
  module HTTP
    Response = Struct.new(:status, :headers, :body) do
      def self.[](status, headers, body) = new(status, headers, body)
    end
    class Headers
      def initialize(fields) = @fields = fields
      def to_a = @fields
      def [](name) = @fields.select { |k, _| k == name }.map(&:last).join(",").then { |v| v.empty? ? nil : v }
    end
    module Body
      Buffered = Struct.new(:chunks, :length)
    end
  end
end

Request = Struct.new(:method, :path, :authority, :headers)
calls = Hash.new(0)
app = lambda do |env|
  calls[env["PATH_INFO"]] += 1
  case env["PATH_INFO"]
  when "/public"
    [200, { "content-type" => "text/css", "cache-control" => "public, max-age=60", "content-length" => "11",
            "set-cookie" => "s=1", "etag" => "\"v1\"" }, ["body-public"]]
  when "/vary"
    [200, { "cache-control" => "public, max-age=60", "vary" => "Accept" }, ["accept=#{env["HTTP_ACCEPT"]}"]]
  else
    [200, { "cache-control" => "private" }, ["dynamic"]]
  end
end
mw = ThrusterCache::Middleware.new(app)
inner = []
front = ThrusterCache::Front.new(->(request) { inner << request.path; :passed_to_app })
req = ->(path, headers = {}, method: "GET") {
  Request.new(method, path, "example.org", Protocol::HTTP::Headers.new(headers.to_a))
}
rack_get = ->(path, extra = {}) {
  mw.call(Rack::MockRequest.env_for("http://example.org#{path}", { "HTTP_HOST" => "example.org" }.merge(extra)))
}
check = ->(cond, what) { raise "FAILED: #{what}" unless cond }

check.(front.call(req.("/public")) == :passed_to_app, "a miss goes to the app behind the front")
rack_get.("/public")
r = front.call(req.("/public"))
check.(r.status == 200 && r.body.chunks == ["body-public"] && r.body.length == 11, "a stored response is answered by the front")
check.(r.headers["x-cache"] == "hit" && r.headers["content-type"] == "text/css", "the front's hit carries the stored headers and X-Cache: hit")
check.(r.headers["set-cookie"].nil?, "the front's hit carries no Set-Cookie")
check.(r.headers["vary"] == "Accept-Encoding", "the front's hit carries the Vary the store added")
check.(r.headers.to_a.none? { |k, _| k == "content-length" }, "the front's hit leaves Content-Length to the writer (a stored one went out twice)")
check.(calls["/public"] == 1, "the front's hit never reaches the Rack app")

r = front.call(req.("/public", { "if-none-match" => "\"v0\", \"v1\"" }))
check.(r.status == 304 && r.body.nil? && r.headers["content-type"].nil? && r.headers["etag"] == "\"v1\"", "the front answers 304 to the ETag")
r = front.call(req.("/public", method: "HEAD"))
check.(inner.last == "/public" && r == :passed_to_app, "HEAD has its own key, so a GET entry is not its hit")
rack_get.("/public", method: "HEAD")
r = front.call(req.("/public", method: "HEAD"))
check.(r.status == 200 && r.body.nil? && r.headers["content-length"] == "11", "a HEAD hit has no body and states its length")

check.(front.call(req.("/public", { "range" => "bytes=0-1" })) == :passed_to_app, "a Range request bypasses the front")
check.(front.call(req.("/public", { "upgrade" => "websocket" })) == :passed_to_app, "a WebSocket upgrade bypasses the front")
check.(front.call(req.("/public", method: "POST")) == :passed_to_app, "a POST bypasses the front")
check.(front.call(req.("/public?x=1")) == :passed_to_app, "the query string is part of the key")

rack_get.("/vary", "HTTP_ACCEPT" => "text/html")
r = front.call(req.("/vary", { "accept" => "text/html" }))
check.(r.status == 200 && r.body.chunks == ["accept=text/html"], "a Vary'd entry is found for matching request headers")
check.(front.call(req.("/vary", { "accept" => "application/json" })) == :passed_to_app, "a different Vary value is not a hit")

ENV["ROUNDHOUSE_RESPONSE_CACHE"] = "0"
check.(front.call(req.("/public")) == :passed_to_app, "ROUNDHOUSE_RESPONSE_CACHE=0 turns the front off too")

puts "ALL OK"
"##;

#[test]
fn falcon_front_answers_hits_from_the_shared_store() {
    let output = Command::new("ruby")
        .args(["-e", FRONT_SCRIPT])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Ruby interpreter available");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "falcon front failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("ALL OK"), "falcon front cases did not execute:\n{stdout}");
}
