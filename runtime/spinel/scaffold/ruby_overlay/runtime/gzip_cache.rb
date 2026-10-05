# frozen_string_literal: true

# Cache of gzip(identity body) for the CRuby overlay.
#
# Rack::Deflater compresses every response. A campfire room page is the
# same ~420 KB HTML for every wrk GET that shares a session, so that is
# the same deflate over and over. Key by the identity bytes: MRI's
# string hash of a 420 KB body is cheaper than SHA-256 of the same
# bytes (measured: digest-keyed cache dropped /rooms/1 from ~1725 to
# ~1140 req/s). The Spinel twin keys by digest because its Hash hashes
# the whole key under the lock and has no GVL.
#
# Gzip itself runs outside the lock. Holding Mutex across Zlib.gzip
# serialized every miss onto one core. Two threads that miss the same
# body both gzip and one write wins — a duplicate deflate, not a
# wrong body.
#
# HTML only, same skips as tep: 1xx/204/304, HEAD, already-encoded,
# small, listed binary types. Wraps run_rack only so /cable's hijack
# tuple never enters here.
require "zlib"

module GzipCache
  MAX_ENTRIES = 64

  @store = {}
  @mutex = Mutex.new

  def self.wrap(app)
    lambda { |env| call(app, env) }
  end

  def self.call(app, env)
    status, headers, body = app.call(env)
    maybe_gzip(env, status, headers, body)
  end

  def self.maybe_gzip(env, status, headers, body)
    return [status, headers, body] if status < 200 || status == 204 || status == 304
    return [status, headers, body] if env["REQUEST_METHOD"] == "HEAD"
    accept = env["HTTP_ACCEPT_ENCODING"].to_s
    return [status, headers, body] unless accepts_gzip?(accept)
    return [status, headers, body] if header(headers, "content-encoding")
    raw = join_body(body)
    return [status, headers, [raw]] if raw.bytesize < 64
    return [status, headers, [raw]] if binary?(header(headers, "content-type"))
    gz = compress(raw)
    headers = headers.dup
    headers["content-encoding"] = "gzip"
    vary = header(headers, "vary")
    if vary.nil? || vary.empty?
      headers["vary"] = "Accept-Encoding"
    elsif !vary.downcase.include?("accept-encoding")
      headers["vary"] = "#{vary}, Accept-Encoding"
    end
    headers["content-length"] = gz.bytesize.to_s
    [status, headers, [gz]]
  end

  def self.compress(raw)
    hit = nil
    @mutex.synchronize { hit = @store[raw] }
    return hit unless hit.nil?
    gz = Zlib.gzip(raw)
    @mutex.synchronize do
      if @store.size >= MAX_ENTRIES
        @store.clear
      end
      @store[raw] = gz
    end
    gz
  end

  def self.join_body(body)
    parts = []
    body.each { |part| parts << part.to_s }
    body.close if body.respond_to?(:close)
    parts.join
  end

  def self.header(headers, name)
    headers[name] || headers[name.split("-").map(&:capitalize).join("-")]
  end

  def self.accepts_gzip?(accept)
    accept.to_s.downcase.split(",").any? { |part|
      coding, *params = part.strip.split(";")
      next false unless coding == "gzip" || coding == "x-gzip"
      q = "1"
      params.each { |p|
        k, v = p.strip.split("=", 2)
        q = v.to_s if k == "q"
      }
      q.to_f > 0.0
    }
  end

  def self.binary?(ct)
    return false if ct.nil? || ct.empty?
    ct.start_with?("image/") || ct.start_with?("audio/") ||
      ct.start_with?("video/") || ct.start_with?("font/") ||
      ct.start_with?("application/octet-stream") ||
      ct.start_with?("application/zip") ||
      ct.start_with?("application/gzip") ||
      ct.start_with?("application/wasm")
  end
end
