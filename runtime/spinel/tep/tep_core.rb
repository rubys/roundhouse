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
