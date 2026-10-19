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
# `reconnect_attempts: 1` does for Sidekiq. Like Sidekiq, that can
# duplicate a job whose first LPUSH reached Redis before the socket
# died. A command error is not retried.
require "redis"

module SidekiqRedis
  CONN = []
  CONN_PID = [0]
  LOCK = Mutex.new

  def self.push(url, queue, json)
    conn = acquire(url)
    begin
      LOCK.synchronize { send_push(conn, queue, json) }
    rescue Redis::BaseConnectionError
      discard(conn)
      conn = acquire(url)
      LOCK.synchronize { send_push(conn, queue, json) }
    end
    nil
  end

  def self.schedule(url, at, json)
    conn = acquire(url)
    begin
      LOCK.synchronize { conn.zadd("schedule", at, json) }
    rescue Redis::BaseConnectionError
      discard(conn)
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
    conn = password == "" ? Redis.new(url: url) : Redis.new(url: url, password: password)
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

  def self.close_quietly(conn)
    begin
      conn.close
    rescue StandardError
      nil
    end
    nil
  end
end
