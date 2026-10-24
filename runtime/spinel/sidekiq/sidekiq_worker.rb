# frozen_string_literal: true
# The `sidekiq` role: the same build, started as `./app sidekiq` (or
# `ruby sidekiq.rb` on ruby and jruby), pulls ActiveJob jobs from Redis
# and runs them, as a Sidekiq 8.1 server process does with open-source
# Sidekiq's BasicFetch. A tree on `queue_adapter :sidekiq` gets this
# file at `runtime/sidekiq_worker.rb`, in place of the façade there
# whose `cli` says the app isn't on Sidekiq (`project::spin_shape` and
# `ruby_family_runtime_files` do the swap, as for sidekiq_redis.rb).
#
#   ./app sidekiq [-c N] [-q queue[,weight]]... [-t seconds]
#
# PROCESS SHAPE. The main thread is the supervisor: it installs the
# signal traps, starts N processor threads, and polls the signal flags
# every half second. Each processor thread has its own Redis connection
# and loops:
#
#   BRPOP queue:a queue:b ... 2       # [] after 2 s, then check QUIET
#   run the job, writing it to the dead set if it fails
#
# Strict order (no weights) sends the queues as given. With weights,
# each fetch shuffles the expanded list (a weight-3 queue three times)
# and drops repeats, so a heavier queue comes first more often, as
# Sidekiq's `queues_cmd` does.
#
# RUNNING A JOB. Only ActiveJob jobs (`Sidekiq::ActiveJob::Wrapper` and
# its older alias) can run here; the payload goes to
# `ActiveJob.perform_parsed`. A job that raises, a job class this binary
# doesn't have, and a native Sidekiq job all fail the way Sidekiq fails
# them, and with no retries until the retry set lands, a failed job
# goes straight to the dead set (`SidekiqEnvelope.with_error`), unless
# it says `retry: false` or `dead: false`. Malformed JSON goes to the
# dead set as it is. The dead write is a pipeline of ZADD and the two
# trims Sidekiq sends (Sidekiq wraps them in MULTI; spinel-redis has no
# MULTI yet).
#
# SIGNALS. TSTP sets QUIET: threads finish their job and stop fetching,
# and a job popped after that is pushed straight back. TERM or INT also
# waits up to `-t` seconds (25 by default) for running jobs, then for
# idle threads to leave their BRPOP (so none takes a job pushed back),
# then pushes the unfinished ones back with RPUSH, unchanged, and exits
# at once.
# TTIN logs what each thread is running. The trap blocks only store an
# Integer: everything else happens in the supervisor's poll.
#
# All Redis traffic goes through `SidekiqRedis` (runtime/sidekiq_redis.rb),
# whose real body is spinel-redis on spinel and redis-rb on ruby and
# jruby, so tests can put a scripted one in its place.
module SidekiqWorker
  DEFAULT_CONCURRENCY = 5
  DEFAULT_TIMEOUT = 25
  FETCH_TIMEOUT = 2
  DEAD_TIMEOUT_SECONDS = 15_552_000
  DEAD_MAX_JOBS = 10_000
  POLL_SECONDS = 0.5
  RECONNECT_SECONDS = 1.0

  # False silences `log`, for tests whose output must not carry times.
  LOGGING = [true]

  CONCURRENCY = [DEFAULT_CONCURRENCY]
  TIMEOUT = [DEFAULT_TIMEOUT]
  # The fetch keys (`queue:<name>`), expanded by weight unless STRICT.
  QUEUES = [""]
  STRICT = [true]

  # Set by the traps: [0] TERM or INT, [1] TSTP, [2] TTIN.
  SIGNALS = [0, 0, 0]
  QUIET = [false]

  # Per processor thread, under LOCK: the queue and text of the job it
  # is running ("" when idle), and when it started.
  LOCK = Mutex.new
  INFLIGHT_QUEUE = [""]
  INFLIGHT_JOB = [""]
  INFLIGHT_SINCE = [0.0]
  RUNNING = [0]
  THREADS = []

  # The control connection (requeue and dead writes), opened lazily and
  # shared under CONTROL_LOCK.
  CONTROL = []
  CONTROL_LOCK = Mutex.new

  USAGE = "usage: sidekiq [-c CONCURRENCY] [-q QUEUE[,WEIGHT]]... [-t TIMEOUT]\n" \
    "  -c, --concurrency INT   processor threads to use (default 5, or RAILS_MAX_THREADS)\n" \
    "  -q, --queue QUEUE[,WEIGHT]  queues to process, with optional weights (default: default)\n" \
    "  -t, --timeout NUM       shutdown timeout in seconds (default 25)\n"

  # ---- command line --------------------------------------------------

  # Read the options, Sidekiq's way. Answers "" or what is wrong.
  def self.parse(argv)
    CONCURRENCY[0] = DEFAULT_CONCURRENCY
    env_threads = ENV["RAILS_MAX_THREADS"].to_s
    CONCURRENCY[0] = env_threads.to_i if env_threads.length > 0 && env_threads.to_i > 0
    TIMEOUT[0] = DEFAULT_TIMEOUT
    names = []
    weights = []
    i = 0
    i = 1 if argv.length > 0 && argv[0] == "sidekiq"
    while i < argv.length
      arg = argv[i]
      value = i + 1 < argv.length ? argv[i + 1] : ""
      if arg == "-h" || arg == "--help"
        return "help"
      elsif (arg == "-c" || arg == "--concurrency") && value.length > 0
        return "-c needs a positive number, not " + value if value.to_i <= 0
        CONCURRENCY[0] = value.to_i
        i += 2
      elsif (arg == "-t" || arg == "--timeout") && value.length > 0
        return "-t needs a number, not " + value if value.to_i < 0 || (value.to_i == 0 && value != "0")
        TIMEOUT[0] = value.to_i
        i += 2
      elsif (arg == "-q" || arg == "--queue") && value.length > 0
        parts = value.split(",")
        names.push(parts[0])
        weights.push(parts.length > 1 ? parts[1].to_i : 0)
        i += 2
      else
        return "unknown option " + arg
      end
    end
    if names.length == 0
      names.push("default")
      weights.push(0)
    end
    set_queues(names, weights)
    ""
  end

  # Sidekiq's Capsule#queues=: each name once per weight (at least once),
  # strict when no queue has a weight.
  def self.set_queues(names, weights)
    QUEUES.clear
    strict = true
    j = 0
    while j < names.length
      w = weights[j]
      strict = false if w != 0
      n = w > 1 ? w : 1
      k = 0
      while k < n
        QUEUES.push("queue:" + names[j])
        k += 1
      end
      j += 1
    end
    STRICT[0] = strict
    if strict
      unique = QUEUES.uniq
      QUEUES.clear
      unique.each { |q| QUEUES.push(q) }
    end
    nil
  end

  # The keys for one BRPOP.
  def self.fetch_keys
    return QUEUES.dup if STRICT[0]
    QUEUES.shuffle.uniq
  end

  # ---- the role -------------------------------------------------------

  # `./app sidekiq ...`: never returns.
  def self.cli(argv)
    problem = parse(argv)
    if problem == "help"
      print USAGE
      SidekiqRedis.hard_exit(0)
    elsif problem.length > 0
      $stderr.print "sidekiq: " + problem + "\n" + USAGE
      SidekiqRedis.hard_exit(1)
    end
    boot
    supervise
    SidekiqRedis.hard_exit(0)
    nil
  end

  # The database pool the worker needs: one connection per processor,
  # plus headroom, unless the environment says otherwise.
  def self.prepare_pool(argv)
    parse(argv)
    if ENV["DATABASE_POOL_SIZE"].to_s.length == 0
      size = CONCURRENCY[0] + 2
      size = 8 if size < 8
      ENV["DATABASE_POOL_SIZE"] = size.to_s
    elsif ENV["DATABASE_POOL_SIZE"].to_s.to_i < CONCURRENCY[0] + 1
      log("WARN", "DATABASE_POOL_SIZE " + ENV["DATABASE_POOL_SIZE"].to_s + " is smaller than concurrency + 1 (" + (CONCURRENCY[0] + 1).to_s + ")")
    end
    nil
  end

  def self.boot
    log("INFO", "Booting Sidekiq-compatible worker, concurrency " + CONCURRENCY[0].to_s +
      ", queues " + QUEUES.uniq.map { |q| q[6, q.length - 6] }.join(",") + (STRICT[0] ? " (strict)" : " (weighted)") +
      ", Redis " + SidekiqPush.redacted_url(SidekiqPush.url))
    install_traps
    start(CONCURRENCY[0])
    nil
  end

  def self.install_traps
    Signal.trap("TERM") { SIGNALS[0] = 1 }
    Signal.trap("INT") { SIGNALS[0] = 1 }
    Signal.trap("TSTP") { SIGNALS[1] = 1 }
    Signal.trap("TTIN") { SIGNALS[2] = 1 }
    nil
  end

  # Start `n` processor threads.
  def self.start(n)
    QUIET[0] = false
    SIGNALS[0] = 0
    SIGNALS[1] = 0
    SIGNALS[2] = 0
    LOCK.synchronize do
      INFLIGHT_QUEUE.clear
      INFLIGHT_JOB.clear
      INFLIGHT_SINCE.clear
      i = 0
      while i < n
        INFLIGHT_QUEUE.push("")
        INFLIGHT_JOB.push("")
        INFLIGHT_SINCE.push(0.0)
        i += 1
      end
      RUNNING[0] = n
    end
    THREADS.clear
    i = 0
    while i < n
      THREADS.push(start_processor(i))
      i += 1
    end
    nil
  end

  def self.start_processor(index)
    Thread.new { processor(index) }
  end

  # The supervisor's loop: act on the signal flags until told to stop,
  # then shut down. Returns once shutdown has requeued what it must.
  def self.supervise
    while true
      sleep POLL_SECONDS
      if SIGNALS[2] == 1
        SIGNALS[2] = 0
        dump_running
      end
      if SIGNALS[1] == 1 && !QUIET[0]
        log("INFO", "Received TSTP, no longer accepting new work")
        QUIET[0] = true
      end
      if SIGNALS[0] == 1
        shutdown
        return nil
      end
    end
    nil
  end

  # TERM: stop fetching, give running jobs up to TIMEOUT seconds, then
  # push back whatever is still running.
  def self.shutdown
    log("INFO", "Shutting down")
    QUIET[0] = true
    deadline = clock + TIMEOUT[0]
    while running > 0 && clock < deadline
      sleep POLL_SECONDS
    end
    # An idle thread may still be parked in BRPOP (up to FETCH_TIMEOUT
    # plus the read deadline). Redis would hand it the jobs pushed back
    # below, and an exit right after would lose them; so the requeue
    # waits for every thread that isn't running a job to leave its fetch.
    # A job such a thread pops meanwhile it pushes back itself.
    idle_deadline = clock + FETCH_TIMEOUT + 2
    while running > busy && clock < idle_deadline
      sleep 0.05
    end
    queues = []
    jobs = []
    LOCK.synchronize do
      i = 0
      while i < INFLIGHT_JOB.length
        if INFLIGHT_JOB[i].length > 0
          queues.push(INFLIGHT_QUEUE[i])
          jobs.push(SidekiqEnvelope.requeue_text(INFLIGHT_JOB[i]))
        end
        i += 1
      end
    end
    if jobs.length > 0
      begin
        with_control { |conn| SidekiqRedis.requeue(conn, queues, jobs) }
        log("INFO", "Pushed " + jobs.length.to_s + " jobs back to Redis")
      rescue StandardError => e
        log("WARN", "Failed to requeue " + jobs.length.to_s + " jobs: " + e.message)
      end
    end
    log("INFO", "Bye!")
    nil
  end

  def self.running
    LOCK.synchronize { RUNNING[0] }
  end

  # How many processor threads are running a job.
  def self.busy
    LOCK.synchronize do
      n = 0
      INFLIGHT_JOB.each { |job| n += 1 if job.length > 0 }
      n
    end
  end

  def self.dump_running
    now = clock
    LOCK.synchronize do
      i = 0
      while i < INFLIGHT_JOB.length
        if INFLIGHT_JOB[i].length > 0
          log("INFO", "thread " + i.to_s + " " + INFLIGHT_QUEUE[i] + " " + describe(INFLIGHT_JOB[i]) +
            " running " + (now - INFLIGHT_SINCE[i]).round(1).to_s + " s")
        end
        i += 1
      end
    end
    nil
  end

  # ---- a processor ------------------------------------------------------

  def self.processor(index)
    conn = []
    down_since = 0.0
    while !QUIET[0]
      popped = []
      begin
        conn.push(SidekiqRedis.worker_open(SidekiqPush.url)) if conn.length == 0
        popped = SidekiqRedis.fetch(conn[0], fetch_keys, FETCH_TIMEOUT)
        if down_since > 0.0
          log("INFO", "Redis is back after " + (clock - down_since).round(1).to_s + " s")
          down_since = 0.0
        end
      rescue StandardError => e
        if down_since == 0.0
          log("WARN", "Redis fetch failed: " + e.message)
          down_since = clock
        end
        if conn.length > 0
          SidekiqRedis.close_quietly(conn[0])
          conn.clear
        end
        sleep RECONNECT_SECONDS
      end
      if popped.length == 2
        queue = popped[0]
        raw = popped[1]
        if QUIET[0]
          push_back(queue, raw)
        else
          LOCK.synchronize do
            INFLIGHT_QUEUE[index] = queue
            INFLIGHT_JOB[index] = raw
            INFLIGHT_SINCE[index] = clock
          end
          run(queue[6, queue.length - 6], raw)
          LOCK.synchronize do
            INFLIGHT_QUEUE[index] = ""
            INFLIGHT_JOB[index] = ""
          end
        end
      end
    end
    SidekiqRedis.close_quietly(conn[0]) if conn.length > 0
    LOCK.synchronize { RUNNING[0] = RUNNING[0] - 1 }
    nil
  end

  def self.push_back(queue, raw)
    begin
      with_control { |conn| SidekiqRedis.requeue(conn, [queue], [raw]) }
    rescue StandardError => e
      log("WARN", "Failed to push a job back to " + queue + ": " + e.message + ": " + raw)
    end
    nil
  end

  # Run one popped job. `queue` is its name without `queue:`.
  def self.run(queue, raw)
    started = clock
    env = nil
    begin
      env = JSON.parse(raw)
    rescue StandardError => e
      log("WARN", "Invalid JSON for job on " + queue + ": " + raw)
      write_dead(raw)
      return nil
    end
    label = describe_env(env)
    unless SidekiqEnvelope.active_job?(env)
      fail_job(env, raw, queue, label, "NameError", "uninitialized constant " + SidekiqEnvelope.klass(env), started)
      return nil
    end
    log("INFO", label + " start")
    payload = SidekiqEnvelope.aj_payload(env)
    outcome = Db.with_connection { ActiveJob.perform_parsed(payload) }
    if outcome == ActiveJob::PERFORM_RAN
      log("INFO", label + " done: " + elapsed(started) + " sec")
    elsif outcome == ActiveJob::PERFORM_DISCARDED
      log("INFO", label + " discarded: " + elapsed(started) + " sec")
    elsif outcome == ActiveJob::PERFORM_UNKNOWN_CLASS
      fail_job(env, raw, queue, label, "NameError", "uninitialized constant " + payload["job_class"].to_s, started)
    else
      fail_job(env, raw, queue, label, ActiveJob.failure_class, ActiveJob.failure_message, started)
    end
    nil
  end

  def self.fail_job(env, raw, queue, label, error_class, message, started)
    log("WARN", label + " fail: " + elapsed(started) + " sec: " + error_class + ": " + message)
    if SidekiqEnvelope.dead?(env)
      write_dead(SidekiqEnvelope.with_error(raw, queue, error_class, message, now_ms))
    else
      log("INFO", label + " dropped: retry is off")
    end
    nil
  end

  # ZADD dead now <job>, then Sidekiq's trims by age and by count. A
  # Redis error loses the job, logged in full, as Sidekiq loses it.
  def self.write_dead(json)
    now = Time.now.to_f
    begin
      with_control do |conn|
        SidekiqRedis.write_dead(conn, now, json, (now - DEAD_TIMEOUT_SECONDS).to_s, (0 - DEAD_MAX_JOBS).to_s)
      end
    rescue StandardError => e
      log("WARN", "Failed to write a job to the dead set: " + e.message + ": " + json)
    end
    nil
  end

  # Run the block on the control connection, opening it if need be and
  # dropping it after a failure.
  def self.with_control
    CONTROL_LOCK.synchronize do
      CONTROL.push(SidekiqRedis.worker_open(SidekiqPush.url)) if CONTROL.length == 0
      begin
        yield CONTROL[0]
      rescue StandardError => e
        SidekiqRedis.close_quietly(CONTROL[0])
        CONTROL.clear
        raise e
      end
    end
    nil
  end

  # ---- logging ----------------------------------------------------------

  def self.describe(raw)
    begin
      describe_env(JSON.parse(raw))
    rescue StandardError
      "(invalid JSON)"
    end
  end

  def self.describe_env(env)
    wrapped = env["wrapped"]
    klass = wrapped.nil? ? SidekiqEnvelope.klass(env) : wrapped.to_s
    "class=" + klass + " jid=" + SidekiqEnvelope.jid(env)
  end

  def self.log(level, message)
    return nil unless LOGGING[0]
    $stdout.print Time.now.utc.iso8601(3) + " pid=" + Process.pid.to_s + " " + level + ": " + message + "\n"
    $stdout.flush
    nil
  end

  def self.clock
    Process.clock_gettime(Process::CLOCK_MONOTONIC)
  end

  def self.elapsed(started)
    (clock - started).round(3).to_s
  end

  def self.now_ms
    Process.clock_gettime(Process::CLOCK_REALTIME, :millisecond)
  end
end
