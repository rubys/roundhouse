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
  # Cumulative collection counters at a phase boundary, one plain line per
  # phase; the wrapper turns consecutive lines into per-phase deltas. This is
  # the same-work guard for any GC-configuration comparison: a ratio between
  # two lanes whose timed loops ran different collection counts is measuring
  # the trigger regime, not the mark (matz/spinel#3513 — a floating-trigger
  # comparison once read -2.8% where the pinned one read -61%). Printed at
  # boundaries the driver already flushes, so a crashed run keeps every
  # completed phase's counters.
  # `remembered`/`remembered_peak` (spinel a02ffd70) are appended AFTER the two
  # counters the wrapper has always parsed, so an older wrapper reading fields
  # 1..3 is unaffected and an older binary that does not carry the keys prints
  # zeros rather than failing. A minor collection walks every entry in that set
  # and runs its scan, so on a store-heavy route with a large old heap the minor
  # can end up doing the full mark's work plus the bookkeeping — the peak
  # against the live set is the first thing to check on a route where the
  # generational configuration LOSES (matz/spinel#3513, /recent).
  def self.gcstat(phase)
    g = GC.stat
    puts "GCSTAT " + phase + " " + g["cycle"].to_s + " " + g["full_runs"].to_s +
         " " + g["remembered"].to_s + " " + g["remembered_peak"].to_s +
         " " + g["bytes"].to_s + " " + g["old_bytes"].to_s + " " + g["str_count"].to_s
  end

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

    # The SQLite build actually linked into THIS binary, asked at runtime.
    # The interpreted lanes run the sqlite3 gem's bundled build; this lane
    # links the system library, and a version gap between the two lands
    # entirely on query-heavy routes (/top profiled at ~94% inside
    # libsqlite3 — matz/spinel#3513). Recorded per lane so the report can
    # attest the lanes queried the same engine, or say loudly that they
    # did not.
    # Low-level statement API on purpose: Db.select_rows lives in the
    # ruby-family adapter layer, which the spinel build does not compile.
    stmt = Db.prepare("select sqlite_version()")
    sqlv = "unknown"
    sqlv = Db.column_value(stmt, 0).to_s if Db.step?(stmt)
    Db.finalize(stmt)
    puts "SQLITE " + sqlv

    # The GC configuration the binary actually ran under, self-reported the
    # way the SQLite build is: the runtime's own reading of the env var, not
    # the harness's belief about what it exported. Mirrors sp_gc's parse
    # since matz/spinel@d3b1400d, which made the generational mark the
    # DEFAULT — minor unless SPINEL_GC_MINOR is set to something starting
    # with "0". The published lane runs the default, so this line is the
    # attestation that no ambient SPINEL_GC_MINOR=0 turned it back into a
    # whole-heap mark; the replay refuses a run that reports otherwise.
    gm = ENV["SPINEL_GC_MINOR"]
    puts "GCCONFIG " + ((!gm.nil? && gm.length > 0 && gm[0] == "0") ? "whole-heap" : "minor")
    gcstat("boot")

    # Baseline: loaded and connected, nothing served yet — the instant the
    # CRuby lanes take BOOT_RSS_KB. Sampled AFTER the seed so the fixture is
    # inside it, matching the CRuby lanes' baseline.
    boot_rss = rss_vmrss_kb

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
    gcstat("parity")
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

    # Phase markers, flushed at each boundary. The driver's stdout is a
    # block-buffered file: without these, a crash after the post-parity
    # flush loses everything in the buffer and the log cannot say which
    # phase died — the 2026-08-03 signal-11 run could only be located to
    # "somewhere in verify, warmup, or the timed loop". One flush per
    # PHASE costs the measurement nothing; per-visit output would.
    puts "VERIFYDONE"
    gcstat("verify")
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
    puts "WARMUPDONE " + warmup_ms.to_s
    gcstat("warmup")
    $stdout.flush

    # Per-visit samples, keyed by raw path and ACCUMULATED IN MEMORY — the
    # printing happens after the timed loop, because a `puts` per visit would
    # put ~7000 writes inside the very section being measured. Templating
    # (/u/alice -> /u/:username) and medians are the wrapper's job: it can
    # reuse the CRuby lane's own table, so the keys join instead of being a
    # second implementation that drifts.
    samples = {}
    # DIAGNOSTIC ONLY, off unless BENCH_REM_PROBE is set: the remembered set's
    # size around each visit, so a per-route figure exists at all. GC.stat's
    # `remembered_peak` is a process-wide high-water mark, which cannot say
    # WHICH route filled the set, and a route run in isolation does not
    # reproduce the mixed sequence's behaviour — so the only way to attribute
    # it is to sample inside the real sequence. This allocates a hash per
    # visit and therefore PERTURBS the timing: runs with it on are for the
    # remembered numbers, never for ms/iter (matz/spinel#3513).
    rem_probe = (ENV["BENCH_REM_PROBE"] || "") != ""
    rem_max = {}
    cyc_n = {}
    ful_n = {}
    vis_n = {}
    iters = 0
    total = 0.0
    while iters < min_iters || (warmup_ms + total) < min_seconds * 1000.0
      t0 = now_ms
      seq.each do |spec|
        p2 = spec.split(" ", 2)
        path = p2[1].to_s
        if rem_probe
          g0 = GC.stat
          c0 = g0["cycle"]
          f0 = g0["full_runs"]
        end
        v0 = now_ms
        visit(jar, p2[0].to_s, path, "")
        if rem_probe
          g1 = GC.stat
          cyc = cyc_n.fetch(path, 0)
          cyc_n[path] = cyc + (g1["cycle"] - c0)
          ful = ful_n.fetch(path, 0)
          ful_n[path] = ful + (g1["full_runs"] - f0)
          vis = vis_n.fetch(path, 0)
          vis_n[path] = vis + 1
        end
        if rem_probe
          # Current size, sampled per visit. This UNDERCOUNTS: a minor
          # collection clears the set and on this workload collections run
          # about once per visit, so the sample sees only what accumulated
          # since the last clear. It is a lower bound, and it is the same
          # bound for every route, so the cross-route comparison is fair even
          # though the absolute numbers are not. (`remembered_peak` cannot
          # substitute here — it is a monotonic process-wide high-water mark,
          # so "the peak after visiting X" only grows with time and
          # attributes nothing to X. For a true per-route upper bound, run
          # the route as its own single-route sequence and read the peak.)
          r = GC.stat["remembered"]
          prev = rem_max.fetch(path, 0)
          rem_max[path] = r if r > prev
        end
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
    if rem_probe
      rem_max.each { |path, r| puts "REM " + path + " " + r.to_s }
      vis_n.each { |path, n| puts "GCV " + path + " " + n.to_s + " " + cyc_n.fetch(path, 0).to_s + " " + ful_n.fetch(path, 0).to_s }
    end
    gcstat("timed")
    puts "RSS " + boot_rss.to_s + " " + rss_vmrss_kb.to_s + " " + rss_vmhwm_kb.to_s
    puts "DONE " + seq.length.to_s + " " + warmup.to_s
    0
  end
end
