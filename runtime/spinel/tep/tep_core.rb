require "digest"
require "zlib"

module Tep
  # The name the server announces itself by. scaffold/main.rb sets
  # Tep::APP.name to the app's own name (the underscored module that
  # wraps its Rails::Application, supplied by ingest) before it starts
  # the server; the binary's file name is the fallback.
  def self.display_name
    n = Tep::APP.name
    n.length > 0 ? n : File.basename($PROGRAM_NAME)
  end

  # What the banner says about OS workers. The runtime reads
  # SPINEL_WORKERS at the first Thread.new and otherwise runs one worker
  # per core; it has no accessor for the effective count, so the banner
  # reports the declaration, never a measurement.
  def self.os_workers_desc
    w = ENV["SPINEL_WORKERS"] || ""
    if w.length == 0
      "one per core (SPINEL_WORKERS=N to cap)"
    else
      w + " (SPINEL_WORKERS)"
    end
  end

  # `--workers N` preforks N processes; the banner names the flag only
  # when it is in effect.
  def self.processes_desc(workers)
    workers > 1 ? workers.to_s + " (--workers)" : "1"
  end

  def self.str_hash
    # Missing-key reads must return "" — the tep readers assume it (parser.rb
    # cookie handling, request.rb Connection/Content-Type, etc.).
    Hash.new("")
  end

  # A byte count off the wire (Content-Length) as an Integer: -1 unless
  # it is a plain run of ASCII digits, 0 for "". Digits only is Puma's
  # rule — it rejects any Content-Length matching /[^\d]/ — so no sign,
  # no junk after the number; `.to_i` let both through ("12abc" read as
  # 12, "-1" as a length that drained nothing). "" is what an absent
  # header reads as through str_hash, and Puma reads an empty value as
  # zero too.
  #
  # Leading zeros are skipped, so a zero-padded value reads as its value
  # (Puma's `.to_i` does the same). More than 18 SIGNIFICANT digits
  # saturates at BYTE_COUNT_CEILING instead of being converted: spinel's
  # Integer is a fixed int64, so a 25-digit length cannot be converted at
  # all. Every 18-digit value is below the ceiling, which is below 2^63.
  #
  # The ceiling means "too large to represent", never a real size, so no
  # comparison may treat it as one: `body_refusal` refuses it whatever
  # the cap, and `max_body_from_env` will not take it as a cap. When the
  # override could saturate too, a zero-padded "…0001024" became a cap of
  # 10^18 and a 25-digit Content-Length compared EQUAL to it and passed.
  BYTE_COUNT_CEILING = 1000000000000000000

  def self.decimal_byte_count(s)
    n = s.bytesize
    i = 0
    while i < n
      b = s.getbyte(i)
      if b < 48 || b > 57
        return -1
      end
      i += 1
    end
    i = 0
    while i < n && s.getbyte(i) == 48
      i += 1
    end
    if n - i > 18
      return BYTE_COUNT_CEILING
    end
    v = 0
    while i < n
      v = v * 10 + (s.getbyte(i) - 48)
      i += 1
    end
    v
  end

  # The largest request body the servers will read, in bytes. Headers
  # were always capped (MAX_REQUEST_BYTES); the body was not, and every
  # drain held the whole of it in one String before the app saw the
  # request — so a `Content-Length: 10737418240` and a stream of bytes
  # grew one worker until it died. A request declaring more than this is
  # answered 413 from its headers, before any of the body is read.
  #
  # 100 MiB by default: tep buffers a body in memory, so this is a
  # per-request memory bound, and it has to clear what the apps upload
  # (campfire attachments arrive as multipart bodies through here, and
  # campfire sets no limit of its own). TEP_MAX_BODY_BYTES overrides it;
  # a value that is not a positive byte count below BYTE_COUNT_CEILING
  # leaves the default — an override too large to represent is not a cap.
  MAX_BODY_BYTES_DEFAULT = 100 * 1024 * 1024

  def self.max_body_from_env
    v = Tep.decimal_byte_count(ENV["TEP_MAX_BODY_BYTES"] || "")
    v > 0 && v < BYTE_COUNT_CEILING ? v : MAX_BODY_BYTES_DEFAULT
  end

  # Read once, at load: the environment does not change under a running
  # server, and this is consulted on every request.
  @max_body_bytes = Tep.max_body_from_env

  def self.max_body_bytes
    @max_body_bytes
  end

  # gzip / x-gzip with q>0. `include?("gzip")` would honour gzip;q=0
  # (RFC 9110 §12.5.3: q=0 means not acceptable) and would match
  # `gzipfoo`. wrk sends `gzip` or `identity`; this is the protocol.
  def self.accepts_gzip?(accept)
    s = accept.downcase
    n = s.bytesize
    i = 0
    while i < n
      while i < n
        b = s.getbyte(i)
        break unless b == 32 || b == 44
        i += 1
      end
      break if i >= n
      name_end = i
      while name_end < n
        b = s.getbyte(name_end)
        break if b == 32 || b == 44 || b == 59
        name_end += 1
      end
      namelen = name_end - i
      gzip = (namelen == 4 && s[i, 4] == "gzip") || (namelen == 6 && s[i, 6] == "x-gzip")
      j = name_end
      while j < n && s.getbyte(j) != 44
        j += 1
      end
      if gzip
        q = Tep.encoding_q(s, name_end, j)
        return true if q > 0
      end
      i = j + 1
    end
    false
  end

  # Quality value of one coding, from the ';' after its name to the
  # comma (or end). Missing q is 1. q=0 / q=0.0 is 0; q=0.8 is 8 tenths
  # so the caller can treat it as > 0 without Float.
  def self.encoding_q(s, from, to)
    j = from
    while j < to
      if s.getbyte(j) == 59
        j += 1
        while j < to && s.getbyte(j) == 32
          j += 1
        end
        if j + 1 < to && s.getbyte(j) == 113 && s.getbyte(j + 1) == 61
          j += 2
          return 0 if Tep.q_is_zero?(s, j, to)
          return 1
        end
      else
        j += 1
      end
    end
    1
  end

  def self.q_is_zero?(s, from, to)
    j = from
    return true if j >= to
    while j < to
      b = s.getbyte(j)
      break if b == 32 || b == 59
      if b == 46
        j += 1
        next
      end
      return false if b < 48 || b > 57
      return false if b != 48
      j += 1
    end
    true
  end

  # Gzip of an identity body, keyed by SHA-256 of the identity bytes.
  # A campfire room page is the same HTML for every wrk GET that shares
  # a session; without this, Zlib.gzip runs on every request and is the
  # measured cliff (1984 → 694 req/s). Keying on the raw 420 KB body
  # hashed and compared that whole string under the lock on every hit;
  # the digest is 64 hex chars and is computed outside the lock.
  #
  # Gzip itself also runs outside the lock. Holding Mutex across
  # Zlib.gzip serialized every miss onto one green thread — Spinel's
  # lane has no GVL, so that was the whole CPU. Two threads that miss
  # the same body both gzip and one write wins.
  #
  # Cap is a COUNT so a bound does not need an LRU touch on the read
  # path. The lock is still required: a green thread can be descheduled
  # inside Hash#[]=.
  GZIP_CACHE_MAX = 64
  GZIP_LOCK = Mutex.new
  @gzip_bodies = Hash.new("")

  def self.gzip_cached(raw)
    key = Digest::SHA256.hexdigest(raw)
    hit = ""
    GZIP_LOCK.synchronize do
      hit = @gzip_bodies[key]
    end
    return hit if hit.length > 0
    gz = Zlib.gzip(raw)
    GZIP_LOCK.synchronize do
      if @gzip_bodies.size >= GZIP_CACHE_MAX
        @gzip_bodies = Hash.new("")
      end
      @gzip_bodies[key] = gz
    end
    gz
  end

  # Honour Accept-Encoding: gzip the way campfire's `use Rack::Deflater`
  # does on CRuby. Inline bodies only — sendfile/streaming/websocket stay
  # as they are. Mutates res.body and stamps Content-Encoding + Vary.
  def self.maybe_gzip!(req, res)
    return if res.streaming || res.upgrading_ws
    return if res.file_path.length > 0
    # Rack::Deflater skips 1xx / 204 / 304 (no entity body) and HEAD
    # (Content-Length must match the GET identity body).
    return if res.status < 200 || res.status == 204 || res.status == 304
    return if req.verb == "HEAD"
    return if res.body.bytesize < 64
    return if res.headers["Content-Encoding"].length > 0
    accept = req.req_headers["accept-encoding"]
    return unless Tep.accepts_gzip?(accept)
    ct = res.headers["Content-Type"]
    return if ct.start_with?("image/") || ct.start_with?("audio/") ||
              ct.start_with?("video/") || ct.start_with?("font/") ||
              ct.start_with?("application/octet-stream") ||
              ct.start_with?("application/zip") ||
              ct.start_with?("application/gzip") ||
              ct.start_with?("application/wasm")
    # DEFAULT_COMPRESSION matches Rack::Deflater. The cache is what
    # recovers the uncompressed cliff: gzip itself is the cost, not
    # Huffman tables (level 1 was measured, same ~700 req/s uncached).
    res.body = Tep.gzip_cached(res.body)
    res.headers["Content-Encoding"] = "gzip"
    vary = res.headers["Vary"]
    if vary.length == 0
      res.headers["Vary"] = "Accept-Encoding"
    elsif !vary.downcase.include?("accept-encoding")
      res.headers["Vary"] = vary + ", Accept-Encoding"
    end
  end

  # Holder for a Fiber so the cooperative scheduler (Tep::Scheduler, the
  # TEP_SERVER=fiber measurement lane) can keep them in a typed array.
  # Spinel's `[Fiber.new { ... }]` array literal infers IntArray (Fiber is
  # a built-in pointer type, not a user class spinel tracks via
  # PtrArray), so a one-attribute wrapper class is the cheapest way to
  # put them in a homogeneous container. Vendored from tep's lib/tep.rb.
  class FiberSlot
    attr_accessor :f
    def initialize(f)
      @f = f
    end
  end

  # A canonical no-op fiber body, used to type-seed Fiber-bearing
  # collections without running anything user-visible.
  def self.seed_fiber_noop
    0
  end

  # Shutdown hook. Tep::Server::Threaded calls Tep.on_shutdown after
  # the accept loop breaks on SIGTERM/SIGINT. Upstream tep fans this
  # out to run_end / Events hooks; roundhouse has none.
  #
  # What it does carry is the collector's attestation, on TEP_GC_STAT=1.
  # matz/spinel#4260's advisory SPINEL_GC_MINOR=1 leg is only evidence
  # if the walk actually COLLECTED: a green compare cannot tell "the
  # write barrier held" from "nothing was ever marked". `remembered_peak`
  # separates them -- 0 with the generational mark off, non-zero with it
  # on and a live remembered set. `full_runs` does not: it counts full
  # SWEEPS (every SP_GC_FULL_INTERVAL cycles), not marks.
  def self.on_shutdown
    v = ENV["TEP_GC_STAT"] || ""
    if v != "" && v != "0"
      g = GC.stat
      puts "tep: GC.stat cycle=" + g["cycle"].to_s +
           " full_runs=" + g["full_runs"].to_s +
           " remembered=" + g["remembered"].to_s +
           " remembered_peak=" + g["remembered_peak"].to_s
      $stdout.flush
    end
    0
  end

  # str_find -- naive substring search returning the int position of
  # `needle` in `s` starting from `start`, or -1 if not found. Callers
  # use `if x < 0` int comparison, which can't narrow against the
  # int|nil that String#index returns under spinel's narrowing model.
  # Vendored from tep's lib/tep.rb (Tep.str_find).
  def self.str_find(s, needle, start)
    nlen = needle.length
    slen = s.length
    pos = start
    while pos <= slen - nlen
      if s[pos, nlen] == needle
        return pos
      end
      pos += 1
    end
    -1
  end
end
