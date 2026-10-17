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
# socket), keyed by pid, and shared by the request threads under one
# lock. A push is one round trip, so the lock is held briefly.
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
    LOCK.synchronize do
      begin
        send_push(connection(url), queue, json)
      rescue Redis::BaseConnectionError
        drop
        send_push(connection(url), queue, json)
      end
    end
    nil
  end

  def self.schedule(url, at, json)
    LOCK.synchronize do
      begin
        connection(url).zadd("schedule", at, json)
      rescue Redis::BaseConnectionError
        drop
        connection(url).zadd("schedule", at, json)
      end
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

  def self.connection(url)
    if CONN_PID[0] != Process.pid || CONN.length == 0
      CONN.clear
      CONN.push(Redis.new(url: url))
      CONN_PID[0] = Process.pid
    end
    CONN[0]
  end

  def self.drop
    if CONN.length > 0 && CONN_PID[0] == Process.pid
      begin
        CONN[0].close
      rescue StandardError
        nil
      end
    end
    CONN.clear
    CONN_PID[0] = 0
    nil
  end
end
