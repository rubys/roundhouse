# frozen_string_literal: true
# Tep::ThrusterCache -- Thruster's response cache, in process, for the
# spinel binary.
#
# A Rails app deployed from its own Dockerfile runs `thrust` in front of
# Puma, and Thruster keeps every response that says `Cache-Control:
# public` with a positive max-age in memory and answers repeats itself:
# campfire's avatar route (`expires_in 30.minutes, public: true`) and
# every `/assets/*` file after its first request. The CRuby trees carry
# the same rules as Rack middleware (scaffold/ruby_overlay/runtime/
# thruster_cache.rb, whose header gives Thruster's sources); this is that
# store for the binary, which has no Rack. Without it the binary ran the
# whole controller -- before_actions, CSP, the render -- for a response
# every other lane on the bench page answers from memory.
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
# A file the app answers with `send_file` is stored as its path, not its
# bytes: the hit is the same sendfile without the app, which is what
# Thruster's copy saves too.
#
# OUTERMOST. Tep::App#dispatch looks here before it leases a database
# connection, as Thruster answers before Puma is reached. One store per
# process: the binary is one process whose OS worker threads share it
# under the mutex.
#
# ROUNDHOUSE_RESPONSE_CACHE=0 turns it off, for A/B measurement.
module Tep
  class ThrusterCacheEntry
    attr_reader :status, :headers, :body, :file_path, :vary_names, :vary_values, :expires_at, :size

    def initialize(status, headers, body, file_path, vary_names, vary_values, expires_at, size)
      @status = status
      @headers = headers
      @body = body
      @file_path = file_path
      @vary_names = vary_names
      @vary_values = vary_values
      @expires_at = expires_at
      @size = size
    end
  end

  class ThrusterCache
    MAX_CACHEABLE_URI = 2048
    MAX_ITEM = 1048576
    ENTRY_OVERHEAD = 256

    attr_reader :capacity, :size

    def initialize(capacity)
      @capacity = capacity
      @size = 0
      @enabled = (ENV["ROUNDHOUSE_RESPONSE_CACHE"] || "") != "0"
      # Insertion-ordered: least recently used first.
      @items = { "" => ThrusterCacheEntry.new(0, Tep.str_hash, "", "", [""], [""], 0.0, 0) }
      @items.delete("")
      @lock = Mutex.new
    end

    def enabled?
      @enabled
    end

    def enabled=(value)
      @enabled = value
    end

    def self.now
      Process.clock_gettime(Process::CLOCK_MONOTONIC)
    end

    # The store key for a request Thruster would look up, or "" when it
    # bypasses the cache.
    def lookup_key(req)
      return "" unless @enabled
      return "" unless req.verb == "GET" || req.verb == "HEAD"
      h = req.req_headers
      return "" if h.fetch("upgrade", "").downcase == "websocket"
      return "" if h.fetch("connection", "") == "Upgrade"
      return "" if h.fetch("range", "").length > 0
      target = req.raw_query.length > 0 ? req.path + "?" + req.raw_query : req.path
      return "" unless target.bytesize < MAX_CACHEABLE_URI
      req.verb + "\n" + target + "\n" + h.fetch("host", "")
    end

    # Answer `req` from the store into `res`. True when it did; false
    # leaves `res` untouched for the app.
    def answer(req, res)
      key = lookup_key(req)
      return false if key.length == 0
      entry = find(key, req, ThrusterCache.now)
      return false if entry.nil?
      ThrusterCache.fill(entry, req, res)
      true
    end

    # After the app ran: store `res` when Thruster would, and mark it.
    def store(req, res)
      key = lookup_key(req)
      if key.length == 0
        res.headers["X-Cache"] = "bypass"
        return nil
      end
      lifetime = ThrusterCache.lifetime(res)
      if lifetime <= 0
        res.headers["X-Cache"] = "miss"
        return nil
      end
      # The miss that stores loses Set-Cookie as the stored copy does.
      res.set_cookies.pop while res.set_cookies.length > 0
      ThrusterCache.drop_header(res.headers, "set-cookie")
      res.headers["Vary"] = "Accept-Encoding" if ThrusterCache.header(res.headers, "vary").length == 0
      now = ThrusterCache.now
      record(store_key(key, req, now), req, res, now + lifetime)
      res.headers["X-Cache"] = "miss"
      nil
    end

    # Lifetime in seconds, or 0 when Thruster would not store it.
    def self.lifetime(res)
      return 0 if res.streaming || res.upgrading_ws
      status = res.status
      return 0 if status < 200 || status > 399 || status == 304
      return 0 if ThrusterCache.header(res.headers, "vary").include?("*")
      cc = ThrusterCache.header(res.headers, "cache-control")
      return 0 if cc.length == 0
      return 0 unless cc.match?(/\bpublic\b/) && !cc.match?(/\bno-cache\b/)
      m = cc.match(/\bs-max-age=(\d+)\b/)
      m = cc.match(/\bmax-age=(\d+)\b/) if m.nil?
      return 0 if m.nil?
      m[1].to_i
    end

    # A header's value by name, any case; "" when absent.
    def self.header(headers, name)
      found = ""
      headers.each do |k, v|
        found = v if k.downcase == name
      end
      found
    end

    def self.drop_header(headers, name)
      doomed = headers.keys.select { |k| k.downcase == name }
      doomed.each { |k| headers.delete(k) }
      nil
    end

    def self.vary_names(headers)
      vary = ThrusterCache.header(headers, "vary")
      names = vary.split(",").map { |n| n.strip.downcase }.reject { |n| n.length == 0 }
      names.sort
    end

    def self.variant_key(key, names, req)
      out = +key
      names.each do |name|
        out << "\n" << name << "=" << req.req_headers.fetch(name, "")
      end
      out
    end

    def self.variant_matches?(entry, req)
      i = 0
      while i < entry.vary_names.length
        return false if req.req_headers.fetch(entry.vary_names[i], "") != entry.vary_values[i]
        i += 1
      end
      true
    end

    # The live entry for the request, found through the plain key's entry
    # when it is Vary'd; nil when there is none.
    def find(key, req, now)
      entry = get(key, now)
      return nil if entry.nil?
      if entry.vary_names.length > 0 && !ThrusterCache.variant_matches?(entry, req)
        return get(ThrusterCache.variant_key(key, entry.vary_names, req), now)
      end
      entry
    end

    # The key a miss is stored under, as `find` would look it up: the
    # plain key, unless the entry there is Vary'd and another variant.
    def store_key(key, req, now)
      entry = get(key, now)
      return key if entry.nil? || entry.vary_names.length == 0
      return key if ThrusterCache.variant_matches?(entry, req)
      ThrusterCache.variant_key(key, entry.vary_names, req)
    end

    def record(key, req, res, expires_at)
      names = ThrusterCache.vary_names(res.headers)
      values = names.map { |name| req.req_headers.fetch(name, "") }
      headers = Tep.str_hash
      size = res.body.bytesize + key.bytesize + ENTRY_OVERHEAD
      res.headers.each do |k, v|
        next if k.downcase == "x-cache"
        headers[k] = v
        size += k.bytesize + v.bytesize
      end
      entry = ThrusterCacheEntry.new(res.status, headers, res.body.dup, res.file_path.dup,
                                     names, values, expires_at, size)
      set(key, entry)
      nil
    end

    # Copy a stored entry into the response the server writes.
    def self.fill(entry, req, res)
      res.set_status(entry.status)
      entry.headers.each { |k, v| res.headers[k] = v }
      res.headers["X-Cache"] = "hit"
      if ThrusterCache.not_modified?(entry, req.req_headers.fetch("if-none-match", ""))
        res.set_status(304)
        ThrusterCache.drop_header(res.headers, "content-type")
        ThrusterCache.drop_header(res.headers, "content-length")
        return nil
      end
      if entry.file_path.length > 0
        res.send_file(entry.file_path)
      else
        res.set_body_if_empty(entry.body)
      end
      nil
    end

    def self.not_modified?(entry, if_none_match)
      etag = ThrusterCache.header(entry.headers, "etag")
      return false if etag.length == 0 || if_none_match.length == 0
      if_none_match.split(",").any? { |candidate| candidate.strip == etag }
    end

    def get(key, now)
      found = nil
      @lock.synchronize do
        if @items.key?(key)
          entry = @items[key]
          @items.delete(key)
          if entry.expires_at < now
            @size -= entry.size
          else
            @items[key] = entry # most recently used last
            found = entry
          end
        end
      end
      found
    end

    def set(key, entry)
      return nil if entry.size > MAX_ITEM || entry.size > @capacity
      @lock.synchronize do
        if @items.key?(key)
          @size -= @items[key].size
          @items.delete(key)
        end
        while @size + entry.size > @capacity && @items.length > 0
          oldest = @items.keys[0]
          @size -= @items[oldest].size
          @items.delete(oldest)
        end
        @items[key] = entry
        @size += entry.size
      end
      nil
    end
  end

  THRUSTER_CACHE = ThrusterCache.new(Tep.decimal_byte_count(ENV["CACHE_SIZE"] || "") > 0 ? Tep.decimal_byte_count(ENV["CACHE_SIZE"] || "") : 67108864)
end
