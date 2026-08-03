# scripts/lobsters-spinel-driver.rb — the spinel lane's in-process replay
# driver, COMPILED INTO the binary under test.
#
# The other lanes drive Main.run_rack from a Ruby script that requires the
# emitted tree. A spinel tree is a native binary, so its driver has to be
# compiled alongside it — but the seam is the same one Tep::Server calls,
# `Main.dispatch(req, res)`, so this measures the same thing the CRuby
# lanes measure: no socket, no HTTP parse, no marshalling.
#
# scripts/bench-lobsters copies this file into the emitted tree and splices
# a `--bench` branch into main.rb before `make build`. It is NOT part of the
# spinel scaffold: every emitted app would otherwise carry a benchmark
# harness it never runs.
#
#   ./build/blog --bench ROUTES SEQUENCE DUMPDIR WARMUP MIN_ITERS MIN_SECONDS SEED
#
# SEED is the fixture file to page-copy into the (in-memory) database BLOG_DB
# names, or "" to serve BLOG_DB as it stands. The interpreted lanes get this
# from scripts/lobsters-replay through the sqlite3 gem; a compiled binary has
# no gem to borrow, so the same seed rides in here.
#
# ROUTES / SEQUENCE are plain text, one "VERB PATH" per line. Output is
# plain lines on stdout; the CRuby wrapper turns them into summary.json.
# Deliberately no JSON on this side — every construct here is compiled, so
# the compiled surface stays as small as the job allows.
#
#   PARITY <route> <status> <bytes> <ms>
#   SEQFAIL <idx> <status> <path>
#   ITER <ms>
#   DONE <visits> <warmup>
#
# Keep the request construction in lockstep with Tep::Parser.parse — a
# hand-built Request that skips a step the parser takes does not fail
# loudly, it fails as a wrong measurement. The parser merges query and
# form fields into req_params and stores DECODED cookie values; missing
# that last one costs the session and turns every authenticated route
# into a cheap 302.
class BenchDriver
  def self.cookie_header(jar)
    out = +""
    jar.each do |k, v|
      out << "; " if out.length > 0
      out << k
      out << "="
      out << v
    end
    out
  end

  # Read Set-Cookie lines with split, never index: under matz/spinel#3400
  # a <<-built string pushed into an ivar Array[String] survives
  # concatenation and split but not every method dispatch, and index is
  # one of the unarmed cells.
  def self.absorb(jar, res)
    res.set_cookies.each do |line|
      pair = line.split(";", 2)[0].to_s
      kv = pair.split("=", 2)
      next if kv.length < 2
      jar[kv[0].to_s] = kv[1].to_s
    end
  end

  def self.visit(jar, verb, path, body)
    req = Tep::Request.new
    req.verb = verb
    req.raw_path = path
    pq = path.split("?", 2)
    req.path = pq[0].to_s
    if pq.length > 1
      req.query = Tep::Url.parse_query(pq[1].to_s)
      req.query.each { |k, v| req.req_params[k] = v }
    end

    hdr = cookie_header(jar)
    if hdr.length > 0
      req.req_headers["cookie"] = hdr
      hdr.split(";").each do |part|
        kv = part.strip.split("=", 2)
        # Parser stores the DECODED value; the wire form is percent-escaped.
        req.cookies[kv[0].to_s] = Tep::Url.unescape(kv[1].to_s) if kv.length == 2
      end
    end
    req.remote_host = "127.0.0.1"

    if body.length > 0
      req.raw_body = body
      req.req_headers["content-type"] = "application/x-www-form-urlencoded"
      req.req_headers["content-length"] = body.length.to_s
      # What Tep::Request#consume_body does after draining the socket.
      Tep::Url.parse_query(body).each { |k, v| req.req_params[k] = v }
    end

    res = Tep::Response.new
    # Same contract as Tep::Server: one request's failure is not the run's.
    # Without this a single unimplemented method (spinel currently lacks
    # Relation#group_by, which /threads and /s/:story_id both reach) unwinds
    # the whole driver and the lane reports NO DATA instead of reporting
    # which routes are broken — the diagnosis being the entire point.
    #
    # The rescue is INSIDE with_connection deliberately. Db.with_connection
    # leases without an `ensure`, so an exception unwinding through it never
    # releases the lease; catching outside it leaks one connection per
    # failing request and the pool then blocks forever in its
    # `while @pool.available == 0` spin. Catching inside keeps the normal
    # release path on every request, failed or not.
    #
    # Exception, not StandardError: the emitted gem facades raise
    # NotImplementedError (a ScriptError), which a bare rescue misses. A
    # measurement harness has to survive everything the app can throw or
    # it reports "no data" for what is really "one route is broken".
    Db.with_connection do
      begin
        Main.dispatch(req, res)
      rescue Exception => e
        res.status = 500
        res.body = "BENCHERR " + e.class.to_s + ": " + e.message.to_s
      end
    end
    absorb(jar, res)
    res
  end

  # The layout emits <meta name="csrf-param" content="authenticity_token">
  # BEFORE the form, so anchor on the hidden input's name+value pair.
  def self.csrf(body)
    parts = body.split("name=\"authenticity_token\" value=\"", 2)
    return "" if parts.length < 2
    parts[1].split("\"", 2)[0].to_s
  end

  def self.login!(jar)
    res = visit(jar, "GET", "/login", "")
    return "GET /login -> " + res.status.to_s if res.status != 200
    token = csrf(res.body)
    return "no authenticity_token in /login" if token.length == 0
    form = "email=" + Tep::Url.escape("wiegand.michell@mertz-vonrueden.test") +
           "&password=" + Tep::Url.escape("ji3W36xR") +
           "&authenticity_token=" + Tep::Url.escape(token)
    res = visit(jar, "POST", "/login", form)
    return "POST /login -> " + res.status.to_s + " (expected 302)" if res.status != 302
    ""
  end

  def self.lines(path)
    out = [""]
    out.pop
    File.read(path).split("\n").each do |l|
      s = l.strip
      out << s if s.length > 0
    end
    out
  end

  def self.now_ms
    Process.clock_gettime(Process::CLOCK_MONOTONIC) * 1000.0
  end

  def self.safe_name(route)
    out = +""
    i = 0
    while i < route.length
      c = route[i]
      ok = (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") ||
           (c >= "0" && c <= "9") || c == "." || c == "-" || c == "_"
      out << (ok ? c : "_")
      i += 1
    end
    # Mirrors the other lanes' rule: leading "/" is dropped, not mapped.
    out.length > 0 && route[0] == "/" ? out[1, out.length - 1].to_s : out
  end

  # Memory, measured the way every other lane measures it: this process
  # reads its own /proc/self/status. One lane is one process, so VmHWM IS
  # this run's peak. Kept deliberately identical to scripts/lobsters-replay
  # so the two report the same statistic from the same source — the AOT lane
  # is a different runtime, not a different measurement.
  #
  # 0 rather than nil when /proc is absent (macOS): the wrapper maps 0 back
  # to null so the JSON matches the other lanes, and a peak is never guessed
  # from a current-RSS reading.
  #
  # File.read, and NOT File.readlines, on purpose. Both are broken on /proc
  # under spinel today (matz/spinel#3411 — st_size is 0 there, and File.read
  # sizes its buffer from stat()), but they fail very differently:
  #
  #   File.read      returns "" — the match finds nothing, this reports 0,
  #                  and the page shows a blank memory row
  #   File.readlines takes the process DOWN — the lane died with SIGSEGV on
  #                  Linux, and hangs locally on a 53-line st_size-0 file
  #
  # A blank row costs one section; a SIGSEGV costs the whole lane, including
  # the timings and parity that have nothing to do with memory. So this takes
  # the silent-and-wrong option deliberately, and says so.
  #
  # Nothing here changes when #3411 lands: File.read starts returning the file,
  # the regex starts matching, and the numbers appear on their own.
  def self.rss_vmrss_kb
    return 0 if !File.exist?("/proc/self/status")
    File.read("/proc/self/status")[/^VmRSS:\s+(\d+) kB/, 1].to_i
  end

  def self.rss_vmhwm_kb
    return 0 if !File.exist?("/proc/self/status")
    File.read("/proc/self/status")[/^VmHWM:\s+(\d+) kB/, 1].to_i
  end

  def self.run(routes_file, seq_file, dump_dir, warmup, min_iters, min_seconds,
               seed_file)
    # In-memory seed, BEFORE the baseline reading and before anything is
    # served. main.rb has already opened BLOG_DB and run the schema DDL; the
    # page-level backup replaces that empty database with the fixture, which
    # is the same order scripts/lobsters-replay uses for the CRuby lanes
    # (boot, then seed, then sample RSS).
    #
    # Seeding here rather than in the emitted app on purpose: an in-memory
    # copy of a file database is a BENCHMARK shape, not something every
    # emitted app should do at boot. The CRuby lane makes the same split —
    # its harness owns the seed, the app owns nothing about it.
    if seed_file.length > 0
      Db.seed_from_file(seed_file)
    end

    # Baseline: loaded and connected, nothing served yet — the instant the
    # CRuby lanes take BOOT_RSS_KB. Sampled AFTER the seed so the fixture is
    # inside it, matching the CRuby lanes' baseline.
    boot_rss = rss_vmrss_kb

    # BALLAST: N retained small objects the request loop never touches, for
    # the mark-cost experiment from matz/spinel#3513 — if collection cost
    # tracks the live heap rather than a request's own allocation, the
    # cheap routes' medians rise with this knob while their code is
    # untouched. 0 (the default, and the published configuration) allocates
    # nothing. The BALLAST line printed after the timed loop is also the
    # read that keeps this array live across the whole run — without it,
    # liveness-based rooting could collect the ballast mid-experiment.
    ballast = []
    bn = (ENV["BENCH_BALLAST"] || "0").to_i
    bi = 0
    while bi < bn
      ballast << ("b" + bi.to_s)
      bi += 1
    end

    jar = Tep.str_hash
    err = login!(jar)
    if err.length > 0
      puts "LOGINFAIL " + err
      return 1
    end

    # ── parity pass: one visit per distinct route, bodies dumped ──────
    lines(routes_file).each do |spec|
      parts = spec.split(" ", 2)
      verb = parts[0].to_s
      route = parts[1].to_s
      t0 = now_ms
      res = visit(jar, verb, route, "")
      ms = now_ms - t0
      File.write(dump_dir + "/" + safe_name(route), res.body)
      puts "PARITY " + route + " " + res.status.to_s + " " +
           res.body.length.to_s + " " + ms.to_s
      puts "PARITYERR " + route + " " + res.body if res.body.start_with?("BENCHERR")
    end

    # Buffered stdout is lost if the process dies later; the parity rows
    # are the diagnosis, so get them out of the buffer now.
    $stdout.flush

    # ── frozen sequence: verify every visit, then time ────────────────
    seq = lines(seq_file)
    idx = 0
    seq.each do |spec|
      parts = spec.split(" ", 2)
      res = visit(jar, parts[0].to_s, parts[1].to_s, "")
      puts "SEQFAIL " + idx.to_s + " " + res.status.to_s + " " + parts[1].to_s if res.status != 200
      idx += 1
    end

    $stdout.flush

    # Warmup wall time feeds the stop clock below — ruby-bench's rule
    # (harness/harness.rb) counts total_time from iteration 1 while
    # discarding the warmup iterations from the stats.
    w = 0
    warmup_ms = 0.0
    while w < warmup
      w0 = now_ms
      seq.each { |spec| p2 = spec.split(" ", 2); visit(jar, p2[0].to_s, p2[1].to_s, "") }
      warmup_ms = warmup_ms + (now_ms - w0)
      w += 1
    end

    # Per-visit samples, keyed by raw path and ACCUMULATED IN MEMORY — the
    # printing happens after the timed loop, because a `puts` per visit would
    # put ~7000 writes inside the very section being measured. Templating
    # (/u/alice -> /u/:username) and medians are the wrapper's job: it can
    # reuse the CRuby lane's own table, so the keys join instead of being a
    # second implementation that drifts.
    samples = {}
    iters = 0
    total = 0.0
    while iters < min_iters || (warmup_ms + total) < min_seconds * 1000.0
      t0 = now_ms
      seq.each do |spec|
        p2 = spec.split(" ", 2)
        path = p2[1].to_s
        v0 = now_ms
        visit(jar, p2[0].to_s, path, "")
        arr = samples.fetch(path, nil)
        if arr.nil?
          arr = []
          samples[path] = arr
        end
        arr << (now_ms - v0)
      end
      ms = now_ms - t0
      puts "ITER " + ms.to_s
      total = total + ms
      iters += 1
    end
    # One line per distinct path: "V <path> <ms> <ms> …". Emitted after the
    # timed loop so it costs the measurement nothing.
    samples.each do |path, arr|
      line = "V " + path
      arr.each { |x| line = line + " " + x.to_s }
      puts line
    end
    puts "RSS " + boot_rss.to_s + " " + rss_vmrss_kb.to_s + " " + rss_vmhwm_kb.to_s
    puts "BALLAST " + ballast.length.to_s
    puts "DONE " + seq.length.to_s + " " + warmup.to_s
    0
  end
end
