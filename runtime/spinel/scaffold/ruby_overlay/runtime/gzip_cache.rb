# frozen_string_literal: true

# Cache of gzip(identity body) for the CRuby overlay.
#
# Rack::Deflater compresses every response. A campfire room page is the
# same ~420 KB HTML for every wrk GET that shares a session, so that is
# the same deflate over and over.
#
# Hit path, measured on 420 KB identity HTML:
#   * `join_body` of Rack `[body]` must not `join` (that copied 420 KB).
#   * Last-identity compare (`bytesize` then `==`) is ~7 µs; MRI string
#     hash of a fresh 420 KB body is ~294 µs. wrk hammers one URL, so
#     the last-hit wins.
#   * The Hash fallback keys by SHA-256 hex of the body (64 chars), not
#     the identity bytes and not CRC32. Holding 64 × 420 KB strings as
#     Hash keys was the RSS cost; CRC32+size could return another body's
#     gzip on a collision. Digest keys keep the store small and refuse
#     wrong-body hits. SHA-256 runs only when last-hit misses.
#
# Gzip itself runs outside the lock. HTML only, same skips as tep.
require "digest"
require "zlib"

module GzipCache
  MAX_ENTRIES = 64

  @store = {}
  @mutex = Mutex.new
  @last_raw = nil
  @last_gz = nil

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
    @mutex.synchronize do
      lr = @last_raw
      if !lr.nil? && lr.bytesize == raw.bytesize && lr == raw
        return @last_gz
      end
    end
    dig = Digest::SHA256.hexdigest(raw)
    hit = nil
    @mutex.synchronize do
      hit = @store[dig]
    end
    return hit unless hit.nil?
    gz = Zlib.gzip(raw)
    @mutex.synchronize do
      if @store.size >= MAX_ENTRIES
        @store.clear
      end
      @store[dig] = gz
      # Snapshot: the Rack body string can be reused and mutated
      # between requests. Sharing it would make last-hit `==` match
      # the mutated bytes while `@last_gz` is still the old gzip.
      @last_raw = raw.dup
      @last_gz = gz
    end
    gz
  end

  def self.join_body(body)
    if body.is_a?(Array)
      n = body.length
      if n == 0
        body.close if body.respond_to?(:close)
        return ""
      end
      if n == 1
        s = body[0].to_s
        body.close if body.respond_to?(:close)
        return s
      end
    end
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
