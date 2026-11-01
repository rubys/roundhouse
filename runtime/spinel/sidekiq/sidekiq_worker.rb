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
#   run the job, writing it to the retry or dead set if it fails
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
# them, through Sidekiq's retry rules (`fail_job`): `retry: false` drops
# the job; otherwise the failure is recorded on it
# (`SidekiqEnvelope.with_error`) and it goes to the retry set, scored
# `count**4 + 15` seconds plus jitter from now, until its `retry` limit
# (`max_retries`, 25 by default) or `retry_for` runs out. Then it goes
# to the dead set, unless it says `dead: false`. Malformed JSON goes to
# the dead set as it is. The dead write is a pipeline of ZADD and the
# two trims Sidekiq sends (Sidekiq wraps them in MULTI; spinel-redis has
# no MULTI yet).
#
# THE SCHEDULER. One more thread with its own connection
# (`SidekiqScheduler`, below) moves due jobs from the `retry` and
# `schedule` sets back onto their queues, as Sidekiq's Poller does in
# every server process: Sidekiq's Lua pop, byte for byte, then the push.
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
  MAX_RETRIES = 25
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

  # The control connection (requeue, retry and dead writes), opened
  # lazily and shared under CONTROL_LOCK.
  CONTROL = []
  CONTROL_LOCK = Mutex.new

  USAGE = "usage: sidekiq [-c CONCURRENCY] [-q QUEUE[,WEIGHT]]... [-t TIMEOUT]\n" \
    "  -c, --concurrency INT   processor threads to use (default: config/sidekiq.yml, RAILS_MAX_THREADS or 5)\n" \
    "  -q, --queue QUEUE[,WEIGHT]  queues to process, with optional weights (default: config/sidekiq.yml or default)\n" \
    "  -t, --timeout NUM       shutdown timeout in seconds (default: config/sidekiq.yml or 25)\n"

  # ---- command line --------------------------------------------------

  # Read the options, Sidekiq's way. Answers "" or what is wrong.
  # Sidekiq's order: the command line, then config/sidekiq.yml (read at
  # compile time into JobRegistry), then RAILS_MAX_THREADS, then the
  # defaults.
  def self.parse(argv)
    CONCURRENCY[0] = DEFAULT_CONCURRENCY
    env_threads = ENV["RAILS_MAX_THREADS"].to_s
    CONCURRENCY[0] = env_threads.to_i if env_threads.length > 0 && env_threads.to_i > 0
    CONCURRENCY[0] = JobRegistry.sidekiq_worker_concurrency if JobRegistry.sidekiq_worker_concurrency > 0
    TIMEOUT[0] = DEFAULT_TIMEOUT
    TIMEOUT[0] = JobRegistry.sidekiq_worker_timeout if JobRegistry.sidekiq_worker_timeout >= 0
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
      JobRegistry.sidekiq_worker_queues.split(" ").each do |word|
        parts = word.split(",")
        names.push(parts[0])
        weights.push(parts.length > 1 ? parts[1].to_i : 0)
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
    SidekiqScheduler.start(JobRegistry.sidekiq_worker_poll_interval_average,
      JobRegistry.sidekiq_worker_scheduled_poll_interval)
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
    # The scheduler stops at QUIET too, after pushing a job it has popped.
    while SidekiqScheduler.running? && clock < idle_deadline
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
    # Valid JSON that is not an object (`null`, `[]`, `1`) is no job:
    # dead as it was popped, like invalid JSON.
    unless env.is_a?(Hash)
      log("WARN", "Job on " + queue + " is not a JSON object: " + raw)
      write_dead(raw)
      return nil
    end
    label = describe_env(env)
    unless SidekiqEnvelope.active_job?(env)
      fail_job(env, raw, queue, label, "NameError", "uninitialized constant " + SidekiqEnvelope.klass(env), started)
      return nil
    end
    log("INFO", label + " start")
    # A job gets its own broadcast log, as `dispatch` gives a request
    # and `ActiveJob.drain` gives a job run in process. A processor
    # thread is never a request, so nothing else clears its log: every
    # message a job broadcast stayed on it, rendered HTML and all
    # (31 MB to 210 MB over 100,000 jobs whose model broadcasts).
    Broadcasts.reset_log!
    # Whatever escapes the run (an envelope without `args`, a connection
    # error) fails the job, as Sidekiq's processor does, rather than
    # killing the processor thread (and, on Spinel, the process).
    begin
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
    rescue StandardError => e
      fail_job(env, raw, queue, label, e.class.name, e.message, started)
    end
    nil
  end

  # Sidekiq's retry rules for a job that failed (`JobRetry#process_retry`).
  def self.fail_job(env, raw, queue, label, error_class, message, started)
    log("WARN", label + " fail: " + elapsed(started) + " sec: " + error_class + ": " + message)
    unless SidekiqEnvelope.retries?(env)
      log("INFO", label + " dropped: retry is off")
      return nil
    end
    now = now_ms
    count = SidekiqEnvelope.next_retry_count(env)
    failed = SidekiqEnvelope.with_error(raw, queue, error_class, message, now)
    if SidekiqEnvelope.exhausted?(env, count, max_retries, now)
      if SidekiqEnvelope.discard_when_exhausted?(env)
        log("INFO", label + " discarded: retries exhausted")
      else
        write_dead(failed)
      end
    else
      delay = SidekiqEnvelope.retry_delay(count)
      write_retry(Time.now.to_f + delay, failed)
    end
    nil
  end

  # `max_retries` from config/sidekiq.yml, else Sidekiq's 25.
  def self.max_retries
    n = JobRegistry.sidekiq_worker_max_retries
    n >= 0 ? n : MAX_RETRIES
  end

  # ZADD retry <when> <job>, one command, as Sidekiq writes it. A Redis
  # error loses the job, logged in full, as Sidekiq loses it.
  def self.write_retry(at, json)
    begin
      with_control { |conn| SidekiqRedis.write_retry(conn, at, json) }
    rescue StandardError => e
      log("WARN", "Failed to write a job to the retry set: " + e.message + ": " + json)
    end
    nil
  end

  # ZADD dead now <job>, then Sidekiq's trims by age and by count
  # (`dead_timeout_in_seconds` and `dead_max_jobs` from
  # config/sidekiq.yml, else 180 days and 10,000). A Redis error loses
  # the job, logged in full, as Sidekiq loses it.
  def self.write_dead(json)
    now = Time.now.to_f
    timeout = JobRegistry.sidekiq_worker_dead_timeout
    timeout = DEAD_TIMEOUT_SECONDS if timeout < 0
    keep = JobRegistry.sidekiq_worker_dead_max_jobs
    keep = DEAD_MAX_JOBS if keep < 0
    begin
      with_control do |conn|
        SidekiqRedis.write_dead(conn, now, json, (now - timeout).to_s, (0 - keep).to_s)
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

# Sidekiq's Poller (`Sidekiq::Scheduled`): one thread, started by the
# worker's boot, that moves due jobs from the `retry` set, then the
# `schedule` set, back onto their queues.
#
#   wait 10 + rand * 5 s (rand * 5 when poll_interval_average is set)
#   until QUIET:
#     for retry, then schedule:
#       while EVALSHA <zpopbyscore> 1 <set> <now> answers a job:
#         SADD queues <q>; LPUSH queue:<q> <job with enqueued_at now>
#     wait the poll interval, randomized
#
# The pop is Sidekiq's Lua script, byte for byte, so its SHA is the one
# Ruby Sidekiq processes load and they share Redis's script cache. On
# NOSCRIPT (Redis restarted, or SCRIPT FLUSH) it loads the script again
# and retries. The pop and the push are two round trips, so a job is
# lost if the process dies between them, as with Sidekiq.
#
# The interval: `poll_interval_average`, else the number of Sidekiq
# processes (SCARD processes, at least 1) times
# `average_scheduled_poll_interval` (5 s), both from config/sidekiq.yml.
# Under 10 processes each wait is uniform within 50% of it either way,
# else between 0 and twice it. This worker doesn't heartbeat into
# `processes` yet, so only Ruby Sidekiq processes are counted.
#
# Waits are sliced, so QUIET stops it within half a second. A Redis
# error ends the cycle with a log line (one per outage) and a fresh
# connection next time. A job whose text isn't a JSON object can't be
# pushed and goes to the dead set as it is.
module SidekiqScheduler
  LUA_ZPOPBYSCORE = "local key, now = KEYS[1], ARGV[1]\n" \
    "local jobs = redis.call(\"zrange\", key, \"-inf\", now, \"byscore\", \"limit\", 0, 1)\n" \
    "if jobs[1] then\n" \
    "  redis.call(\"zrem\", key, jobs[1])\n" \
    "  return jobs[1]\n" \
    "end\n"
  SETS = ["retry", "schedule"]
  INITIAL_WAIT = 10
  AVERAGE_SCHEDULED_POLL_INTERVAL = 5.0

  SHA = [""]
  # [0] poll_interval_average, [1] average_scheduled_poll_interval;
  # -1.0 when config/sidekiq.yml gives none.
  INTERVALS = [-1.0, -1.0]
  RUNNING = [false]
  THREAD = []

  def self.start(poll_interval_average, scheduled_poll_interval)
    INTERVALS[0] = poll_interval_average
    INTERVALS[1] = scheduled_poll_interval
    RUNNING[0] = true
    THREAD.clear
    THREAD.push(Thread.new { run })
    nil
  end

  def self.running?
    RUNNING[0]
  end

  def self.run
    conn = []
    down = [false]
    pause(initial_wait)
    while !SidekiqWorker::QUIET[0]
      begin
        conn.push(SidekiqRedis.worker_open(SidekiqPush.url)) if conn.length == 0
        enqueue(conn[0])
        if down[0]
          SidekiqWorker.log("INFO", "Scheduler: Redis is back")
          down[0] = false
        end
      rescue StandardError => e
        SidekiqWorker.log("WARN", "Scheduler: " + e.message) unless down[0]
        down[0] = true
        if conn.length > 0
          SidekiqRedis.close_quietly(conn[0])
          conn.clear
        end
      end
      pause(down[0] ? SidekiqWorker::RECONNECT_SECONDS : interval(conn))
    end
    SidekiqRedis.close_quietly(conn[0]) if conn.length > 0
    RUNNING[0] = false
    nil
  end

  # One cycle: every due job in `retry`, then in `schedule`.
  def self.enqueue(conn)
    SETS.each do |set|
      while !SidekiqWorker::QUIET[0]
        job = zpop(conn, set)
        break if job.nil?
        push(conn, set, job)
      end
    end
    nil
  end

  # Sidekiq's `zpopbyscore`: the first job due by now, removed, or nil.
  def self.zpop(conn, set)
    SHA[0] = SidekiqRedis.script_load(conn, LUA_ZPOPBYSCORE) if SHA[0] == ""
    now = Time.now.to_f.to_s
    job = nil
    begin
      job = SidekiqRedis.evalsha(conn, SHA[0], set, now)
    rescue StandardError => e
      raise e unless e.message.include?("NOSCRIPT")
      SHA[0] = SidekiqRedis.script_load(conn, LUA_ZPOPBYSCORE)
      job = SidekiqRedis.evalsha(conn, SHA[0], set, now)
    end
    job
  end

  def self.push(conn, set, job)
    queue = ""
    begin
      env = JSON.parse(job)
      queue = env["queue"].to_s if env.is_a?(Hash)
    rescue StandardError
      queue = ""
    end
    if queue == ""
      SidekiqWorker.log("WARN", "Scheduler: a job in " + set + " has no queue; it goes to the dead set: " + job)
      SidekiqWorker.write_dead(job)
      return nil
    end
    # The pop already took the job out of the set: if the push fails, the
    # log keeps its text, and `run` still sees the error as an outage.
    begin
      SidekiqRedis.push_on(conn, queue, SidekiqEnvelope.with_enqueued_at(job, SidekiqWorker.now_ms))
    rescue StandardError => e
      SidekiqWorker.log("WARN", "Scheduler: failed to push a job from " + set + " to " + queue + ": " + e.message + ": " + job)
      raise e
    end
    nil
  end

  def self.initial_wait
    (INTERVALS[0] > 0.0 ? 0.0 : INITIAL_WAIT.to_f) + 5.0 * rand
  end

  # Sidekiq's `random_poll_interval`.
  def self.interval(conn)
    count = 1
    average = INTERVALS[0]
    if average <= 0.0
      count = process_count(conn)
      per = INTERVALS[1] > 0.0 ? INTERVALS[1] : AVERAGE_SCHEDULED_POLL_INTERVAL
      average = count * per
    end
    count < 10 ? average * rand + average / 2.0 : average * rand * 2.0
  end

  def self.process_count(conn)
    return 1 if conn.length == 0
    begin
      n = SidekiqRedis.scard(conn[0], "processes")
      n > 0 ? n : 1
    rescue StandardError
      1
    end
  end

  # Sleep `seconds` in half-second slices, leaving early on QUIET.
  def self.pause(seconds)
    left = seconds
    while left > 0.0 && !SidekiqWorker::QUIET[0]
      step = left < SidekiqWorker::POLL_SECONDS ? left : SidekiqWorker::POLL_SECONDS
      sleep step
      left -= step
    end
    nil
  end
end
