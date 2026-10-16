# frozen_string_literal: true

# Thruster's response cache, in process, for the CRuby overlay.
#
# Campfire's Dockerfile runs `thrust` in front of Puma, and Thruster keeps
# every response that says `Cache-Control: public` with a positive
# max-age in memory and answers repeats itself: the avatar route
# (`expires_in 30.minutes, public: true`) and every `/assets/*` file
# (`config.public_file_server.headers`) after their first request. The
# rails lane on the bench page gets that; the emitted tree served each
# repeat through the app again.
#
# IN PROCESS, NOT A PROXY. A proxy is a second process on the same cores
# and a second hop per request: Thruster's own cache hits top out near
# 35k req/s on the bench box (the rails lane's stylesheet), half of what
# the in-process Ruby port of campfire serves the same file at. The port
# (tobi/campfire-once-ruby-ractor, lib/campfire/front.rb) carries
# Thruster's rules into the app process; so does this, after it.
#
# Thruster's rules (internal/cache_handler.go, memory_cache.go):
#   * GET/HEAD only; not a WebSocket upgrade, not a Range request, and a
#     path+query of at most 2,048 bytes;
#   * stored when the status is 2xx/3xx other than 304, Cache-Control
#     says `public` with a positive `s-max-age` or `max-age` and no
#     `no-cache`, and Vary is not `*`; kept for that many seconds;
#   * a stored response loses its Set-Cookie (so does the miss that
#     stored it), and gains `Vary: Accept-Encoding` if it had no Vary;
#   * a Vary'd response is keyed by the request's values for those
#     headers, found through the entry at the plain key;
#   * a hit answers 304 to an If-None-Match naming its ETag;
#   * `X-Cache: hit | miss | bypass` on everything;
#   * items up to 1 MiB, 64 MiB in all (CACHE_SIZE), least recently used
#     evicted first.
#
# One store per worker PROCESS, as the port keeps one per worker Ractor,
# with capacity split across WEB_CONCURRENCY when the environment says how
# many there are. A worker's threads share it under a mutex.
#
# ROUNDHOUSE_RESPONSE_CACHE=0 turns it off, for A/B measurement.
#
# TWO FRONTS, ONE STORE. `Middleware` is the Rack form (Puma). `Front` is
# the same lookup as a Protocol::HTTP app, for Falcon (falcon.rb): a hit is
# answered before Falcon's Rack adapter runs, because building a Rack env
# and converting the response back cost ~14 us a request on the bench box
# against ~3 us for the hit itself. A miss falls through to the adapter,
# where `Middleware` stores it. Both read the request through
# `lookup(method, path, host, header)`, `header` taking a lowercase name.
#
# Not `ResponseCache`: campfire's own app/models/response_cache.rb is a
# top-level class of that name, and the two would collide at boot.
module ThrusterCache
  MAX_CACHEABLE_URI = 2048
  MAX_ITEM = 1 << 20
  ENTRY_OVERHEAD = 256
  CAPACITY = Integer(ENV.fetch("CACHE_SIZE", (64 << 20).to_s)) /
    [ENV.fetch("WEB_CONCURRENCY", "1").to_i, 1].max
  HIT = "hit"
  MISS = "miss"
  BYPASS = "bypass"

  Entry = Struct.new(:status, :headers, :fields, :body, :variant, :expires_at, :size)

  @items = {} # insertion-ordered: least recently used first
  @size = 0
  @mutex = Mutex.new

  def self.enabled?
    ENV["ROUNDHOUSE_RESPONSE_CACHE"] != "0"
  end

  # Rack middleware; outermost, so it answers ahead of the static file
  # server as Thruster does ahead of Puma.
  class Middleware
    def initialize(app)
      @app = app
    end

    def call(env)
      ThrusterCache.call(@app, env)
    end
  end

  def self.call(app, env)
    method = env["REQUEST_METHOD"]
    header = ->(name) { env["HTTP_#{name.upcase.tr("-", "_")}"] }
    query = env["QUERY_STRING"].to_s
    path = query.empty? ? env["PATH_INFO"].to_s : "#{env["PATH_INFO"]}?#{query}"
    key = lookup_key(method, path, env["HTTP_HOST"].to_s, header)
    return mark(app.call(env), BYPASS) unless key
    now = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    entry, key = lookup(key, now, header)
    return hit(entry, env) if entry

    status, headers, body = app.call(env)
    return [status, headers, body] if status.to_i < 0 # hijacked
    lifetime = cache_lifetime(status, headers)
    return mark([status, headers, body], MISS) unless lifetime

    stored = {}
    headers.each { |name, value| stored[name.downcase] = value }
    stored.delete("set-cookie")
    stored["vary"] = "Accept-Encoding" unless stored["vary"]
    text = read_body(body)
    response_headers = stored.merge("x-cache" => MISS)
    return [status, response_headers, body] if text.nil?
    record(key, header, status, stored, text, now + lifetime)
    [status, response_headers, [text]]
  end

  # The store key for a request Thruster would look up, or nil when it
  # bypasses the cache. `path` carries the query string.
  def self.lookup_key(method, path, host, header)
    return nil unless method == "GET" || method == "HEAD"
    return nil unless enabled?
    return nil if header.call("upgrade").to_s.downcase == "websocket"
    return nil if header.call("connection").to_s == "Upgrade"
    return nil unless header.call("range").to_s.empty?
    return nil unless path.bytesize < MAX_CACHEABLE_URI
    "#{method}\n#{path}\n#{host}"
  end

  # [entry, key]: the live entry for the request (nil when none), found
  # through the plain key's entry when it is Vary'd, and the key it is (or
  # would be) stored under.
  def self.lookup(key, now, header)
    entry = get(key, now)
    if entry && entry.variant && !variant_matches?(entry.variant, header)
      key = variant_key(key, entry.variant, header)
      entry = get(key, now)
    end
    [entry, key]
  end

  def self.variant_key(key, variant, header)
    variant.reduce(+key) { |k, (name, _)| k << "\n" << name << "=" << header.call(name).to_s }
  end

  def self.variant_matches?(variant, header)
    variant.all? { |name, value| header.call(name).to_s == value }
  end

  # Lifetime in seconds, or nil when Thruster would not store it.
  def self.cache_lifetime(status, headers)
    status = status.to_i
    return nil if status < 200 || status > 399 || status == 304
    vary = header(headers, "vary")
    return nil if vary&.include?("*")
    cc = header(headers, "cache-control") or return nil
    return nil unless cc.match?(/\bpublic\b/) && !cc.match?(/\bno-cache\b/)
    m = cc.match(/\bs-max-age=(\d+)\b/) || cc.match(/\bmax-age=(\d+)\b/) or return nil
    seconds = m[1].to_i
    seconds > 0 ? seconds : nil
  end

  def self.header(headers, name)
    headers[name] || headers[name.split("-").map(&:capitalize).join("-")]
  end

  # The whole body, or nil when it is known too large to store: an Array
  # (still servable as it is) or a file it never opened. An iterated body
  # is read whole, and `set` refuses it if it turns out too large.
  def self.read_body(body)
    if body.is_a?(Array)
      text = body.length == 1 ? body[0].to_s : body.join
      body.close if body.respond_to?(:close)
      return text.bytesize <= MAX_ITEM ? text : nil
    end
    if body.respond_to?(:to_path) && File.file?(body.to_path) && File.size(body.to_path) > MAX_ITEM
      return nil
    end
    parts = []
    body.each { |part| parts << part.to_s }
    body.close if body.respond_to?(:close)
    parts.join
  end

  def self.record(key, header, status, headers, text, expires_at)
    variant = vary_names(headers)&.map { |name| [name, header.call(name).to_s].freeze }&.freeze
    size = text.bytesize + key.bytesize + ENTRY_OVERHEAD
    headers.each { |name, value| size += name.bytesize + value.to_s.bytesize }
    fields = headers.filter_map { |name, value| [name, value.to_s].freeze unless WRITER_FIELDS.include?(name) }.freeze
    set(key, Entry.new(status, headers.freeze, fields, text.frozen? ? text : text.dup.freeze, variant, expires_at, size))
  end

  def self.vary_names(headers)
    vary = headers["vary"]
    return nil if vary.nil? || vary.empty?
    vary.split(",").map! { |n| n.strip.downcase }.sort!
  end

  def self.hit(entry, env)
    headers = entry.headers.merge("x-cache" => HIT)
    status = entry.status
    body = entry.body
    if not_modified?(entry, env["HTTP_IF_NONE_MATCH"])
      status = 304
      body = nil
      headers.delete("content-type")
      headers.delete("content-length")
    end
    body = nil if env["REQUEST_METHOD"] == "HEAD"
    [status, headers, body.nil? || body.empty? ? [] : [body]]
  end

  def self.not_modified?(entry, if_none_match)
    etag = entry.headers["etag"] or return false
    return false if if_none_match.nil? || if_none_match.empty?
    if_none_match.split(",").any? { |candidate| candidate.strip == etag }
  end

  HIT_FIELD = ["x-cache", HIT].freeze
  BODILESS = %w[content-type content-length].freeze
  # Fields a Protocol::HTTP response must not carry: Falcon's HTTP/1
  # writer states the length from the body and owns the connection, so a
  # stored `content-length` went out twice and wrk dropped the connection
  # (Falcon's Rack adapter strips the same set from a Rack response).
  WRITER_FIELDS = %w[content-length transfer-encoding connection keep-alive].freeze

  # The hit as [status, fields, body] for a Protocol::HTTP front: fields a
  # fresh Array of [name, value] pairs, body a String or nil.
  def self.hit_parts(entry, method, if_none_match)
    if not_modified?(entry, if_none_match)
      fields = entry.fields.reject { |name, _| BODILESS.include?(name) }
      fields << HIT_FIELD
      return [304, fields, nil]
    end
    fields = entry.fields.dup
    fields << HIT_FIELD
    body = entry.body
    # Without a body the writer cannot state the length, so a HEAD says it.
    return [entry.status, fields << ["content-length", body.bytesize.to_s], nil] if method == "HEAD"
    [entry.status, fields, body.empty? ? nil : body]
  end

  # Falcon's front: answers a hit as a Protocol::HTTP response, without the
  # Rack adapter; anything else goes to `app` (the adapter around
  # config.ru, whose `Middleware` stores what is cacheable). Needs
  # protocol-http, which Falcon brings; nothing here loads it.
  class Front
    def initialize(app)
      @app = app
    end

    def call(request)
      headers = request.headers
      header = ->(name) { headers[name]&.to_s }
      method = request.method
      key = ThrusterCache.lookup_key(method, request.path, request.authority || headers["host"].to_s, header)
      return @app.call(request) unless key
      entry, = ThrusterCache.lookup(key, Process.clock_gettime(Process::CLOCK_MONOTONIC), header)
      return @app.call(request) unless entry
      status, fields, body = ThrusterCache.hit_parts(entry, method, header.call("if-none-match"))
      ::Protocol::HTTP::Response[status, ::Protocol::HTTP::Headers.new(fields),
        body && ::Protocol::HTTP::Body::Buffered.new([body], body.bytesize)]
    end

    def close
      @app.close if @app.respond_to?(:close)
    end
  end

  def self.mark(response, value)
    status, headers, body = response
    return response if status.to_i < 0 # hijacked
    begin
      headers["x-cache"] = value
    rescue FrozenError
      headers = headers.merge("x-cache" => value)
    end
    [status, headers, body]
  end

  def self.get(key, now)
    @mutex.synchronize do
      entry = @items[key] or return nil
      if entry.expires_at < now
        @items.delete(key)
        @size -= entry.size
        return nil
      end
      @items.delete(key)
      @items[key] = entry # most recently used last
      entry
    end
  end

  def self.set(key, entry)
    return if entry.size > MAX_ITEM || entry.size > CAPACITY
    @mutex.synchronize do
      if (old = @items.delete(key))
        @size -= old.size
      end
      while @size + entry.size > CAPACITY && (oldest = @items.each_key { |k| break k })
        @size -= @items.delete(oldest).size
      end
      @items[key] = entry
      @size += entry.size
    end
  end
end
