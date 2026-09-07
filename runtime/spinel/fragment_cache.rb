# Synchronized `Rails.cache` for the spinel binary, overriding the
# shared runtime's unsynchronized store (runtime/ruby/rails.rb).
#
# WHY ONLY HERE. `runtime/ruby` is transpiled to nine targets and
# `Mutex` is not a construct it can spell — a lock there needs an arm in
# every emitter. It does not need one on most of them: the
# single-threaded lanes have nothing to guard, and rust and go already
# put a class-level slot behind their own lock per access. The lane with
# real parallelism is this one — `Tep::Server::Threaded` runs a green
# thread per connection on an M:N scheduler with NO GVL — and a green
# thread can be descheduled at a safepoint in the middle of a Hash
# insert. Two of them resizing `@entries` at once is not a stale read,
# it is a corrupt table. So the lock lives beside the threading, which
# is the two-layer runtime's whole shape (csrf_token.rb is the same
# seam: a reopen that wins by load order).
#
# The CRuby overlay deliberately has NO twin. Puma is threaded too, but
# MRI holds the GVL across `Hash#[]=`, so the same store is already
# atomic there. JRuby is not, and its lane is smoke-only.
#
# SHARDED, AND THE FIRST CUT WAS NOT — the note it replaced said "one
# lock, not shards, until something measures otherwise", and the very
# next benchmark measured otherwise. A single `STORE_LOCK` is taken once
# per FRAGMENT, so ~40+ times on the room page, by every connection at
# once. On the published run (20260907-085458 vs -011858, the same
# binary, the two lanes differing only in `SPINEL_WORKERS`):
#
#   workers  /rooms/1 req/s   /rooms/1/messages req/s
#   1        58 -> 221 (3.8x) 57  -> 389 (6.8x)
#   auto=12  188 -> 181 (.96) 192 -> 215 (1.1x)
#
# The ratios are not the tell. The SCALING is: before the cache, 12
# workers bought 3.2x and 3.4x over one; after it, 12 workers ran at
# 0.82x and 0.55x OF ONE WORKER. Adding cores made it slower, and p99
# went 206 -> 914 ms and 161 -> 1360 ms. That is a convoy, and on this
# runtime it is worse than a plain one: a contended `Mutex` parks the
# green thread, and waking it goes through the one-condvar broadcast in
# sp_sched.c that [[project_blog_bench_spinel_threaded_regression_2026_09_03]]
# already identified as the source of a 600 ms tail. The CRuby lane took
# the same lowering and improved on BOTH axes (8.3x/18.7x, p99 4-16x
# better) because MRI's GVL makes that contention nearly free.
#
# So: N independent sub-stores, each with its own Mutex, chosen by key.
# Same reasoning and same fix as [[project_db_pool_sharding]] — one pool
# meant one mutex on every request, and splitting it was worth 3.3-3.8x
# on this very page.
#
# A read-then-write PAIR is still not atomic, and that is deliberate:
# holding a lock across the render would serialize every request on the
# page's first fragment. Two threads that miss the same key both render
# and one write wins — the cost is a duplicate render, which is what the
# cache was avoiding anyway, not a wrong answer.
#
# Reopens the three methods that touch the store's state and nothing
# else: `fetch_str` is defined in terms of them, `fetch` never touches
# it, and the no-op `read`/`write`/`exist?` have no state to guard.
# `initialize` is NOT reopened — the shards are class-level, built once
# at load, so the shared runtime keeps sole ownership of instance setup
# and this file cannot drift from it.
module Rails
  class Cache
    # More shards than the box has OS workers (autodetect is one per
    # core; this box is 6 physical / 12 logical), so two workers
    # colliding on one shard is the exception rather than the steady
    # state.
    SHARD_COUNT = 32

    # Built with a `while` loop rather than `Array.new(n) { … }` or
    # `(0...n).map`: a block that constructs per-element state is the
    # kind of shape the AOT lane types poorly, and this runs once at
    # load where clarity costs nothing.
    SHARD_LOCKS = []
    SHARD_ENTRIES = []
    SHARD_EXPIRES = []
    SHARD_KEYS = []
    i = 0
    while i < SHARD_COUNT
      SHARD_LOCKS.push(Mutex.new)
      SHARD_ENTRIES.push({})
      SHARD_EXPIRES.push({})
      SHARD_KEYS.push([])
      i = i + 1
    end

    # Per-shard entry cap, so the total stays what the shared runtime
    # documents (`MAX_ENTRIES`) rather than that times the shard count.
    # At least one, so a small cap cannot round to a store that evicts
    # everything it writes.
    SHARD_MAX = (MAX_ENTRIES / SHARD_COUNT) > 0 ? (MAX_ENTRIES / SHARD_COUNT) : 1

    # Which shard owns a key. Read from the TAIL, because that is where
    # our keys differ: a fragment key is
    # `views/<scope><view>/<table>/<id>-<updated_at>`, so the leading
    # bytes are identical across every message on a page and the id and
    # timestamp are at the end. Hashing the first bytes would put a
    # whole room's messages in one shard, which is the thing this
    # exists to avoid.
    def shard_of(k)
      n = k.bytesize
      return 0 if n == 0
      ((k.getbyte(n - 1) * 31) + k.getbyte(n / 2)) % SHARD_COUNT
    end

    def read_str(key)
      k = key.to_s
      s = shard_of(k)
      entries = SHARD_ENTRIES[s]
      hit = nil
      expired = false
      SHARD_LOCKS[s].synchronize do
        if entries.key?(k)
          due = SHARD_EXPIRES[s][k]
          if due == 0 || due > Time.now.to_i
            hit = entries[k]
          else
            expired = true
          end
        end
      end
      # The eviction of an expired entry takes the lock again rather
      # than upgrading inside the read: `forget` takes it itself, and a
      # nested `synchronize` on a non-reentrant Mutex is a deadlock, not
      # a slow path.
      forget(k) if expired
      hit
    end

    def write_str(key, value, ttl)
      k = key.to_s
      s = shard_of(k)
      entries = SHARD_ENTRIES[s]
      expires = SHARD_EXPIRES[s]
      keys = SHARD_KEYS[s]
      SHARD_LOCKS[s].synchronize do
        keys.push(k) unless entries.key?(k)
        entries[k] = value
        expires[k] = ttl > 0 ? Time.now.to_i + ttl : 0
        while keys.length > SHARD_MAX
          oldest = keys.shift
          entries.delete(oldest)
          expires.delete(oldest)
        end
      end
      value
    end

    def forget(k)
      s = shard_of(k)
      SHARD_LOCKS[s].synchronize do
        SHARD_ENTRIES[s].delete(k)
        SHARD_EXPIRES[s].delete(k)
        SHARD_KEYS[s].delete(k)
      end
      nil
    end
  end
end
