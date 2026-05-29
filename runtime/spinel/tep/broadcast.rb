# Tep::Broadcast -- in-process pub-sub topic broker.
#
# Foundation of the Broadcast battery (Battery 2 in
# docs/BATTERIES-DESIGN.md). Apps + later batteries (Presence,
# LiveView) layer on top: WebSocket connections subscribe to
# topics; publish(topic, payload) writes payload to every
# subscribed fd.
#
# Public API:
#
#   sub_id = Tep::Broadcast.subscribe(topic, fd)
#   Tep::Broadcast.publish(topic, payload)
#   Tep::Broadcast.unsubscribe(sub_id)
#   Tep::Broadcast.unsubscribe_fd(fd)    # drop ALL subs for an fd
#
# Subscription model is fd-based rather than block/callback-based
# (spinel can't reliably round-trip blocks-as-values across module
# boundaries, see memory [[spinel_widening_dispatch]]). The
# concrete v1 use case is "deliver to a WS connection" -- the WS
# layer keeps its accepted-socket fd, calls subscribe, and
# Tep::Broadcast.publish writes the payload bytes to that fd.
# Apps that need a different delivery surface (HTTP SSE, log
# fan-out) use the same subscribe-fd shape with a different fd.
#
# Storage scope is per-process: subscriptions live on Tep::APP,
# which under prefork is per-worker. Cross-worker pub-sub goes
# through PG LISTEN/NOTIFY (Tep::Broadcast.enable_pg_backend) --
# subscribers always register fd-local; publish() additionally
# NOTIFY's the configured channel so peer workers' local
# subscribers see the message too.
#
# `subscribe` returns an opaque subscription id (the registry
# index at insertion time). Callers can pass it back to
# `unsubscribe` for a single-sub drop. For WS connections that
# subscribe to multiple topics, `unsubscribe_fd(fd)` drops every
# subscription tied to that fd in one call -- the right shape for
# the WS on-close hook.
module Tep
  module Broadcast
    # Register a subscription for `fd` on `topic`. Returns an
    # opaque sub_id for later unsubscribe. The fd receives raw
    # bytes on publish -- suits SSE / log fan-out / anything that
    # doesn't need WebSocket framing. For WS connections, prefer
    # subscribe_ws.
    def self.subscribe(topic, fd)
      subs = Tep::APP.broadcast_subs
      sub = Tep::BroadcastSubscription.new(topic, fd, 0)
      subs.push(sub)
      subs.length - 1
    end

    # WebSocket-bridged variant of subscribe. The fd is expected
    # to be an established WS connection (typically a
    # Tep::WebSocket::Connection's #fd). On publish, payload is
    # wrapped in a WS TEXT frame via Tep::WebSocket::Driver
    # before delivery -- the peer sees a well-formed WS message,
    # not raw bytes that would close the connection.
    #
    # Cleanup is automatic: when the WS connection closes,
    # Tep::WebSocket::Connection.dispatch_close runs the user's
    # on_close handler and then calls unsubscribe_fd(driver.fd),
    # dropping every subscription tied to the closed connection.
    # Apps don't need to add their own unsubscribe; if they do,
    # the second call just finds 0 matches (harmless).
    def self.subscribe_ws(topic, fd)
      subs = Tep::APP.broadcast_subs
      sub = Tep::BroadcastSubscription.new(
        topic, fd, Tep::WebSocket::OPCODE_TEXT)
      subs.push(sub)
      subs.length - 1
    end

    # Drop the subscription at `sub_id`. Note that ids are
    # registry indexes; subsequent drops shift everything past it
    # downward. For multi-sub drop, prefer `unsubscribe_fd`.
    def self.unsubscribe(sub_id)
      subs = Tep::APP.broadcast_subs
      if sub_id < 0 || sub_id >= subs.length
        return 0
      end
      subs.delete_at(sub_id)
      0
    end

    # Drop every subscription whose fd matches. Returns the count
    # dropped. Used by WS on-close to clean up everything a closing
    # connection had subscribed to. Back-to-front so delete_at
    # indices stay valid mid-loop.
    def self.unsubscribe_fd(fd)
      subs = Tep::APP.broadcast_subs
      dropped = 0
      i = subs.length - 1
      while i >= 0
        if subs[i].fd == fd
          subs.delete_at(i)
          dropped += 1
        end
        i -= 1
      end
      dropped
    end

    # Write `payload` to every subscribed fd for `topic`. Returns
    # the number of subscriptions matched (NOT the number of
    # successful writes -- a closed / bad fd still counts as
    # matched; the underlying sphttp_write_str returns -1 silently
    # on that fd). Apps that need delivery confirmation should
    # track their own ack channel.
    #
    # Roundhouse vendors the local-only fan-out: the cross-worker PG
    # LISTEN/NOTIFY backend from upstream tep is dropped (the blog
    # runs single-worker — WORKERS=1 — so in-process delivery reaches
    # every subscriber). publish is therefore a straight alias for
    # publish_local_only.
    def self.publish(topic, payload)
      Tep::Broadcast.publish_local_only(topic, payload)
    end

    # Total subscription count across all topics. Useful for
    # diagnostics and the v1 test surface.
    def self.subscriber_count
      Tep::APP.broadcast_subs.length
    end

    # Count of subscribers for one topic. O(n) over the registry;
    # acceptable for v1 (n is typically small per worker).
    def self.subscribers_for(topic)
      subs = Tep::APP.broadcast_subs
      n = 0
      i = 0
      while i < subs.length
        if subs[i].topic == topic
          n += 1
        end
        i += 1
      end
      n
    end

    # Drop every subscription. Used by tests between fixtures, and
    # available to apps that need to fully reset (e.g. during
    # graceful shutdown). Returns the count dropped.
    def self.clear
      subs = Tep::APP.broadcast_subs
      n = subs.length
      while subs.length > 0
        subs.delete_at(0)
      end
      n
    end

    # Local fan-out. (Upstream tep names this publish_local_only to
    # distinguish it from the PG-NOTIFY-augmented publish; roundhouse
    # dropped the PG backend, so publish is a straight alias for this.)
    #
    # Branches on each subscription's `mode`:
    #   * mode 0 -> raw bytes via Sock.sphttp_write_str (default,
    #     for SSE / log fan-out / non-framed consumers).
    #   * mode != 0 -> WebSocket frame via Tep::WebSocket::Driver.send_frame,
    #     using the mode value as the WS opcode (1=TEXT, 2=BINARY).
    def self.publish_local_only(topic, payload)
      subs = Tep::APP.broadcast_subs
      matched = 0
      i = 0
      while i < subs.length
        if subs[i].topic == topic
          if subs[i].mode == 0
            Sock.sphttp_write_str(subs[i].fd, payload)
          else
            Tep::WebSocket::Driver.send_frame(
              subs[i].fd, subs[i].mode, payload)
          end
          matched += 1
        end
        i += 1
      end
      matched
    end
  end
end
