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
# ONE LOCK, NOT SHARDS, until something measures otherwise. The DB pool
# is sharded because one pool meant one mutex on every REQUEST held
# across a query ([[project_db_pool_sharding]]); this one is held across
# a Hash lookup and an Array push, tens of nanoseconds, and matz's
# uncontended-Mutex fast path (fc866b04) makes the uncontended case
# nearly free. Shard it when a profile says the room page contends on
# it, not before.
#
# A read-then-write PAIR is still not atomic, and that is deliberate:
# holding the lock across the render would serialize every request on
# the page's first fragment. Two threads that miss the same key both
# render and one write wins — the cost is a duplicate render, which is
# what the cache was avoiding anyway, not a wrong answer.
#
# Reopens the three methods that touch the store's state and nothing
# else: `fetch_str` is defined in terms of them, `fetch` never touches
# it, and the no-op `read`/`write`/`exist?` have no state to guard.
module Rails
  class Cache
    STORE_LOCK = Mutex.new

    def read_str(key)
      k = key.to_s
      hit = nil
      expired = false
      STORE_LOCK.synchronize do
        if @entries.key?(k)
          due = @expires_at[k]
          if due == 0 || due > Time.now.to_i
            hit = @entries[k]
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
      STORE_LOCK.synchronize do
        @keys.push(k) unless @entries.key?(k)
        @entries[k] = value
        @expires_at[k] = ttl > 0 ? Time.now.to_i + ttl : 0
        while @keys.length > MAX_ENTRIES
          oldest = @keys.shift
          @entries.delete(oldest)
          @expires_at.delete(oldest)
        end
      end
      value
    end

    def forget(k)
      STORE_LOCK.synchronize do
        @entries.delete(k)
        @expires_at.delete(k)
        @keys.delete(k)
      end
      nil
    end
  end
end
