# Tep::Broadcast -- in-process fan-out.
#
# A registry of (topic, fd, mode) subscriptions on the Tep::APP
# singleton, and `publish`, which writes a payload to every fd
# subscribed to a topic. WebSocket subscribers (mode = the WS opcode)
# get a framed message; mode 0 gets raw bytes, for SSE / log fan-out.
#
# THREADS. Connections are green threads now (Tep::Server::Threaded), so
# a subscribe on one connection races an unsubscribe or a publish on
# another. Every registry read and write happens under
# `Tep::APP.broadcast_lock`, and `publish` copies the matching
# subscriptions out under the lock and writes OUTSIDE it: a write to a
# subscriber with a full buffer parks this thread (sp_net parks on
# EAGAIN), and it must not park while holding the registry.
#
# A WebSocket subscriber is written through its Driver, never by fd
# number: between the copy and the write the connection can close and
# its fd number can already belong to the next accept. The driver's
# write lock refuses once the connection has retired it
# (Tep::WebSocket::Driver#write_frame), and serialises this publish
# against the connection's own frames.
#
# Roundhouse vendors the local-only fan-out: the cross-worker PG
# LISTEN/NOTIFY backend from upstream tep is dropped (the blog runs
# single-worker -- WORKERS=1 -- so in-process delivery reaches every
# subscriber). `publish` is therefore a straight alias for
# `publish_local_only`.
module Tep
  module Broadcast
    # Subscribe `fd` to `topic` for raw-bytes delivery. Returns the
    # subscription's index at the time of the push (an id for
    # `unsubscribe`, which callers rarely use -- the WS path
    # unsubscribes by fd on close).
    def self.subscribe(topic, fd)
      sub = Tep::BroadcastSubscription.new(
        topic, fd, 0, Tep::WebSocket::Driver.new(fd))
      n = 0
      Tep::APP.broadcast_lock.synchronize do
        subs = Tep::APP.broadcast_subs
        subs.push(sub)
        n = subs.length - 1
      end
      n
    end

    # Subscribe a WebSocket connection: payloads go out as TEXT frames
    # through its driver.
    def self.subscribe_ws(topic, ws)
      sub = Tep::BroadcastSubscription.new(
        topic, ws.fd, Tep::WebSocket::OPCODE_TEXT, ws)
      n = 0
      Tep::APP.broadcast_lock.synchronize do
        subs = Tep::APP.broadcast_subs
        subs.push(sub)
        n = subs.length - 1
      end
      n
    end

    # Drop one subscription by index. Out-of-range is a no-op.
    def self.unsubscribe(sub_id)
      Tep::APP.broadcast_lock.synchronize do
        subs = Tep::APP.broadcast_subs
        if sub_id >= 0 && sub_id < subs.length
          subs.delete_at(sub_id)
        end
      end
      0
    end

    # Drop every subscription of `fd` to `topic`. Returns the count
    # dropped. Back-to-front so delete_at indices stay valid mid-loop.
    def self.unsubscribe_topic_fd(topic, fd)
      dropped = 0
      Tep::APP.broadcast_lock.synchronize do
        subs = Tep::APP.broadcast_subs
        i = subs.length - 1
        while i >= 0
          if subs[i].fd == fd
            if subs[i].topic == topic
              subs.delete_at(i)
              dropped += 1
            end
          end
          i -= 1
        end
      end
      dropped
    end

    # Drop every subscription whose fd matches. Returns the count
    # dropped. Used by WS on-close to clean up everything a closing
    # connection had subscribed to -- called BEFORE the fd is closed,
    # while its number still names only that connection.
    def self.unsubscribe_fd(fd)
      dropped = 0
      Tep::APP.broadcast_lock.synchronize do
        subs = Tep::APP.broadcast_subs
        i = subs.length - 1
        while i >= 0
          if subs[i].fd == fd
            subs.delete_at(i)
            dropped += 1
          end
          i -= 1
        end
      end
      dropped
    end

    # Write `payload` to every subscribed fd for `topic`. Returns the
    # number of subscriptions matched (NOT the number of successful
    # writes -- a closed / bad fd still counts as matched; the underlying
    # write returns -1 silently on that fd). Apps that need delivery
    # confirmation should track their own ack channel.
    def self.publish(topic, payload)
      Tep::Broadcast.publish_local_only(topic, payload)
    end

    # Total subscription count across all topics. Useful for
    # diagnostics and the v1 test surface.
    def self.subscriber_count
      n = 0
      Tep::APP.broadcast_lock.synchronize do
        n = Tep::APP.broadcast_subs.length
      end
      n
    end

    # Count of subscribers for one topic. O(n) over the registry;
    # acceptable for v1 (n is typically small per worker).
    def self.subscribers_for(topic)
      n = 0
      Tep::APP.broadcast_lock.synchronize do
        subs = Tep::APP.broadcast_subs
        i = 0
        while i < subs.length
          if subs[i].topic == topic
            n += 1
          end
          i += 1
        end
      end
      n
    end

    # Drop every subscription. Used by tests between fixtures, and
    # available to apps that need to fully reset (e.g. during
    # graceful shutdown). Returns the count dropped.
    def self.clear
      n = 0
      Tep::APP.broadcast_lock.synchronize do
        subs = Tep::APP.broadcast_subs
        n = subs.length
        subs.clear
      end
      n
    end

    # Local fan-out. The matching subscriptions are copied out under the
    # lock; the writes happen outside it (see the module comment).
    #
    # Branches on each subscription's `mode`:
    #   * mode 0 -> raw bytes via Sock.sphttp_write_bytes (default,
    #     for SSE / log fan-out / non-framed consumers).
    #   * mode != 0 -> WebSocket frame via the subscriber's
    #     Tep::WebSocket::Driver#write_frame, using the mode value as the
    #     WS opcode (1=TEXT, 2=BINARY). A retired driver refuses the
    #     write (-1), which still counts as matched.
    def self.publish_local_only(topic, payload)
      matched = []
      Tep::APP.broadcast_lock.synchronize do
        subs = Tep::APP.broadcast_subs
        i = 0
        while i < subs.length
          if subs[i].topic == topic
            matched.push(subs[i])
          end
          i += 1
        end
      end
      i = 0
      while i < matched.length
        sub = matched[i]
        if sub.mode == 0
          # write_bytes: write_str is strlen-terminated, and a raw
          # payload is not guaranteed NUL-free.
          Sock.sphttp_write_bytes(sub.fd, payload, payload.bytesize)
        else
          sub.ws.write_frame(sub.mode, payload)
        end
        i += 1
      end
      matched.length
    end
  end
end
