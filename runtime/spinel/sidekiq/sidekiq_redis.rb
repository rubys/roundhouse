# frozen_string_literal: true
# The Redis connection `SidekiqPush` pushes through, for a tree on
# `queue_adapter :sidekiq`. Swapped in for the raising façade at
# `runtime/sidekiq_redis.rb`. `require "redis"` is spinel-redis on
# spinel and the redis-rb gem on ruby and jruby; both take
# `Redis.new(url:)`, `pipelined` and `zadd` with the same meaning and
# raise `Redis::BaseConnectionError` for a socket that fails, so one
# body serves every lane.
#
# One connection per process: opened at the first push (after a
# prefork or a Puma fork, so a child never writes on its parent's
# socket), keyed by pid, and shared by the request threads. LOCK covers
# the commands on that one socket, held for a push's single round trip.
# The connect itself happens outside LOCK, so a Redis host that is slow
# to answer holds up only the threads waiting for a connection, not
# every push. Two threads that race to connect each open one; the first
# installed is kept and the other closed.
#
# One reconnect: on a connection error the connection is dropped, a new
# one opened, and the push sent once more, as redis-client's
# `reconnect_attempts: 1` does for Sidekiq. That covers the first
# connect too: a push that can't connect tries once more before it
# raises, as redis-client's `ensure_connected` does. Like Sidekiq, the
# retry can duplicate a job whose first LPUSH reached Redis before the
# socket died. A command error is not retried.
#
# Connect and read timeouts, at redis-client's 1 s defaults that Sidekiq
# runs with: a Redis that stops answering costs a request one second
# (two, with the reconnect) instead of hanging it. Running out raises
# `Redis::CannotConnectError` or `Redis::TimeoutError`, both
# `BaseConnectionError`s, so a timed-out read takes the reconnect above.
require "redis"

module SidekiqRedis
  CONNECT_TIMEOUT = 1.0
  READ_TIMEOUT = 1.0
  CONN = []
  CONN_PID = [0]
  LOCK = Mutex.new

  # `used` holds the connection the first attempt got, if it got one:
  # a connect that fails leaves nothing to discard.
  def self.push(url, queue, json)
    used = []
    begin
      conn = acquire(url)
      used.push(conn)
      LOCK.synchronize { send_push(conn, queue, json) }
    rescue Redis::BaseConnectionError
      discard(used[0]) if used.length > 0
      conn = acquire(url)
      LOCK.synchronize { send_push(conn, queue, json) }
    end
    nil
  end

  def self.schedule(url, at, json)
    used = []
    begin
      conn = acquire(url)
      used.push(conn)
      LOCK.synchronize { conn.zadd("schedule", at, json) }
    rescue Redis::BaseConnectionError
      discard(used[0]) if used.length > 0
      conn = acquire(url)
      LOCK.synchronize { conn.zadd("schedule", at, json) }
    end
    nil
  end

  def self.send_push(conn, queue, json)
    conn.pipelined do |p|
      p.sadd("queues", queue)
      p.lpush("queue:" + queue, json)
    end
    nil
  end

  # This process's connection, connecting outside LOCK when there is
  # none yet.
  def self.acquire(url)
    pid = Process.pid
    held = LOCK.synchronize { current(pid) }
    if held.length == 0
      fresh = open(url, SidekiqPush.password)
      held = LOCK.synchronize { install(pid, fresh) }
    end
    held[0]
  end

  # A new connection, connected and authenticated here, outside LOCK:
  # redis-rb connects at its first command, so the PING makes that
  # happen now rather than inside LOCK at the first push (spinel-redis
  # connects in its constructor already). A `configure_client`
  # `password:` overrides the URL's, as the explicit option does on both
  # clients.
  def self.open(url, password)
    conn = password == "" ? Redis.new(url: url, connect_timeout: CONNECT_TIMEOUT, read_timeout: READ_TIMEOUT) : Redis.new(url: url, password: password, connect_timeout: CONNECT_TIMEOUT, read_timeout: READ_TIMEOUT)
    begin
      conn.ping
    rescue StandardError => e
      close_quietly(conn)
      raise e
    end
    conn
  end

  # Under LOCK: the connection as a zero- or one-element Array, after
  # forgetting one a parent process opened.
  def self.current(pid)
    if CONN_PID[0] != pid
      CONN.clear
      CONN_PID[0] = pid
    end
    CONN.dup
  end

  # Under LOCK: keep `fresh` unless another thread installed one first.
  def self.install(pid, fresh)
    if CONN_PID[0] == pid && CONN.length > 0
      close_quietly(fresh)
    else
      CONN.clear
      CONN.push(fresh)
      CONN_PID[0] = pid
    end
    CONN.dup
  end

  # Drop `conn` after a connection error, unless another thread already
  # replaced it. Under LOCK, so no push is using the socket as it closes.
  def self.discard(conn)
    LOCK.synchronize do
      if CONN.length > 0 && CONN[0].equal?(conn)
        CONN.clear
        close_quietly(conn)
      end
    end
    nil
  end

  # ---- the worker (runtime/sidekiq_worker.rb) --------------------------
  #
  # The worker holds its own connections, one per processor thread and
  # one for control, so these take the connection rather than sharing
  # CONN. A blocking BRPOP's read deadline is its own timeout plus
  # READ_TIMEOUT on both clients, as redis-rb sets it.

  def self.worker_open(url)
    open(url, SidekiqPush.password)
  end

  # BRPOP: [key, value], or [] when the timeout passes (redis-rb answers
  # nil there; spinel-redis answers []).
  def self.fetch(conn, keys, timeout)
    popped = conn.brpop(keys, timeout: timeout)
    popped.nil? ? [] : popped
  end

  # The jobs back on the right of their queues, where BRPOP takes them
  # next: one RPUSH per queue, the jobs in the order given, in one round
  # trip, as Sidekiq's bulk_requeue sends them.
  def self.requeue(conn, keys, jobs)
    order = []
    i = 0
    while i < keys.length
      order.push(keys[i]) unless order.include?(keys[i])
      i += 1
    end
    conn.pipelined do |p|
      order.each do |key|
        batch = []
        j = 0
        while j < keys.length
          batch.push(jobs[j]) if keys[j] == key
          j += 1
        end
        p.rpush(key, batch)
      end
    end
    nil
  end

  # ---- the scheduler -----------------------------------------------------

  # SCRIPT LOAD: the script's SHA1.
  def self.script_load(conn, source)
    conn.script(:load, source)
  end

  # EVALSHA <sha> 1 <key> <arg>: the script's reply, a String or nil.
  # A script the server doesn't hold raises a command error whose
  # message starts with NOSCRIPT, on both clients.
  def self.evalsha(conn, sha, key, arg)
    conn.evalsha(sha, [key], [arg])
  end

  # SCARD processes: the Sidekiq processes that heartbeat.
  def self.scard(conn, key)
    conn.scard(key)
  end

  # A due job pushed back onto its queue, as Sidekiq's client pushes:
  # SADD queues and LPUSH queue:<name> in one round trip.
  def self.push_on(conn, queue, json)
    send_push(conn, queue, json)
  end

  # ZADD retry: the failed job, due at `at` (a Float epoch).
  def self.write_retry(conn, at, json)
    conn.zadd("retry", at, json)
    nil
  end

  # ZADD dead, then the trims by age (scores up to `oldest`) and by
  # count (ranks 0..`keep`, a negative String).
  def self.write_dead(conn, now, json, oldest, keep)
    conn.pipelined do |p|
      p.zadd("dead", now, json)
      p.zremrangebyscore("dead", "-inf", oldest)
      p.zremrangebyrank("dead", "0", keep)
    end
    nil
  end

  # Leave at once, with processor threads still parked: no at_exit, no
  # ensure, as Sidekiq's process ends after its requeue.
  def self.hard_exit(code)
    $stdout.flush
    exit!(code)
  end

  def self.close_quietly(conn)
    begin
      conn.close
    rescue StandardError
      nil
    end
    nil
  end
end
