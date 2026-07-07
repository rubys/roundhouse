# Tep::RedisFeed -- cross-process Redis pub/sub feeding the in-process
# Tep::Broadcast registry. The skeleton of the Mastodon streaming
# architecture: Rails/Sidekiq publish to Redis channels from other
# processes; one RedisFeed fiber per worker subscribes and fans each
# message out to this worker's connection fds (WS-framed or raw/SSE)
# via Tep::Broadcast.publish.
#
# OPT-IN: deliberately not required from tep.rb. It depends on the
# `redis` spin package (github.com/rubys/spinel-redis) being on the
# compiler's -I path; apps that want a Redis-fed broadcast require
# "redis" and this file explicitly. Apps that don't never compile it.
#
# Usage (inside a Tep app's boot, before the scheduler loop starts):
#
#   feed = Tep::RedisFeed.new("127.0.0.1", 6379)
#   feed.subscribe("timeline:1")     # repeatable; psubscribe for patterns
#   feed.spawn                       # parks a fiber on the redis fd
#
# The fiber wakes only when the subscription socket is readable
# (Tep::Scheduler.io_wait), drains every complete push, publishes each
# message payload to the Broadcast topic named by its Redis channel
# (pmessage: the concrete channel, not the pattern), and parks again.
# A lost Redis connection raises out of the fiber: the fiber dies and
# alive_count drops -- supervision/reconnect policy belongs to the app
# (Mastodon's streaming Node restarts on ioredis errors; a port does
# the equivalent at its own layer).
module Tep
  class RedisFeed
    def initialize(host, port)
      @ps = RedisPubSub.new(RedisTransport.new(host, port))
      @listener = RedisListener.new
      @listener.message do |channel, payload|
        Tep::Broadcast.publish(channel, payload)
      end
      @listener.pmessage do |pattern, channel, payload|
        Tep::Broadcast.publish(channel, payload)
      end
    end

    # The listener is exposed so embedders can hook lifecycle events
    # (on subscribe-confirmed, on unsubscribe) without replacing the
    # message handlers wired above.
    def listener
      @listener
    end

    def pubsub
      @ps
    end

    def subscribe(channel)
      @ps.subscribe_start(channel)
    end

    def psubscribe(pattern)
      @ps.psubscribe_start(pattern)
    end

    # Park-drain loop; runs forever inside a scheduler fiber. io_wait
    # returns 0 on timeout -- loop and park again (the timeout only
    # bounds how stale a shutdown check can get).
    def run_loop
      while true
        ready = Tep::Scheduler.io_wait(@ps.fd, Tep::Scheduler::READ, 5)
        if ready > 0
          @ps.drain(@listener)
        end
      end
    end

    # Spawn the feed as a scheduler fiber. Same closure idiom as
    # Server::Scheduled's accept/connection fibers.
    def spawn
      feed = self
      f = Fiber.new { feed.run_loop }
      Tep::Scheduler.spawn_fiber(f)
    end
  end
end
