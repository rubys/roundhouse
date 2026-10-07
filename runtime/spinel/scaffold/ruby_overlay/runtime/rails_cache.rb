# frozen_string_literal: true

# CRuby overlay: a REAL in-memory cache store behind `Rails.cache`,
# replacing the shared runtime's no-op (runtime/rails.rb — correct but
# recomputes every fetch). Lobsters caches its heaviest work through
# Rails.cache (`users_tree_*` 24h, front-page `stories *` 45s, `story *`
# 60s, per-user `unread_replies` 2min), so a no-op store isn't a missing
# optimization, it's a different program than the one Rails runs.
#
# Semantics mirror ActiveSupport::Cache::MemoryStore:
#   * DupCoder: bare Strings are dup'd on write AND read (mutation-safe
#     without Marshal cost — cached page fragments are big strings);
#     everything else Marshal round-trips so each hit gets a fresh object
#     graph (a cached AR row mutated by one request must not leak into
#     the next).
#   * `expires_in:` accepts Integer seconds or ActiveSupport::Duration
#     (both appear in lobsters); expiry checked lazily on read.
#
# Process-local by design: the CRuby serving shape is one process (Puma
# workers=0), matching MemoryStore's own scope. Thread-safety: 32 shards
# keyed the same way as Spinel's `runtime/spinel/fragment_cache.rb`
# (`shard_of` hashes the tail bytes of fragment keys so a room page's
# messages fan out instead of colliding on a shared `views/…` prefix).
# Writes / RMW / eviction take the per-shard Mutex; `read_str` of a
# live frozen String is RCU — Hash#[] under the MRI GVL is atomic, and
# each entry is replaced as a whole `[encoded, expires_at]` pair, so a
# reader never sees a torn value/expiry mix and the hot fragment path
# does not serialize every Puma thread on one process lock.
#
# CRuby-only (overlay, not runtime/ruby): Marshal/Mutex/Time-based
# eviction are exactly the is_a?-dispatching dynamic shapes the shared
# runtime's typing bar excludes; other targets keep the no-op until
# their lobsters turn.
#
# TWO STORES ANSWER `Rails.cache`, and every method the compiler emits
# a call to has to exist on BOTH — this one and the shared runtime's
# `Rails::Cache`. `read_str`/`write_str` landed on the shared one first
# and every campfire page 500'd here with `undefined method 'read_str'
# for an instance of Rails::MemoryStore`, which is the cache's version
# of the dual-runtime parity rule the Db shims carry
# ([[feedback_dual_runtime_parity]]).
module Rails
  def self.cache
    @cache_store ||= MemoryStore.new
  end

  class MemoryStore
    # Match Spinel's fragment store: more shards than typical Puma
    # threads, same hash so a room page's keys spread the same way.
    SHARD_COUNT = 32

    def initialize
      @shards = []
      @mutexes = []
      i = 0
      while i < SHARD_COUNT
        @shards.push({})
        @mutexes.push(Mutex.new)
        i += 1
      end
    end

    def fetch(key, opts = {})
      k = key.to_s
      s = shard_of(k)
      entry = @shards[s][k]
      return decode(entry[0]) if entry && !expired?(entry)
      value = yield
      write(key, value, opts)
      value
    end

    # The typed seam `src/lower/rails_cache.rs` rewrites provably-String
    # fetch sites to, so one lowering serves every ruby-family target.
    # Here it is a thin delegate: this store already holds any value and
    # dups Strings on both sides, which is what the typed half gives up.
    def fetch_str(key, ttl, &block)
      fetch(key, ttl.to_i > 0 ? { expires_in: ttl.to_i } : {}, &block)
    end

    # The BLOCK-FREE half of the same seam, which a view's `<% cache %>`
    # lowers to (`lower::view_to_library::walker`): read, render into
    # the site's own accumulator on a miss, write. A block there would
    # have to capture the accumulator, and on the AOT lane a captured
    # block dissolves into a heap poly proc (matz/spinel#4245).
    #
    # Thin delegates, as `fetch_str` above is — `write` still dups the
    # String into the store. `read_str` does NOT dup on the way out:
    # a view's `<% cache %>` only appends the hit (`io << hit`), and a
    # campfire room page is ~40 message fragments. DupCoder's read-side
    # copy was 40 extra 2–5 KB allocations per wrk GET that shared
    # nothing with mutation safety, because the stored copy is already
    # isolated by the write-side dup. A non-String under that key is a
    # MISS rather than a TypeError at the append, and the write that
    # follows corrects it. `read` (the untyped half) still dups, so a
    # caller that mutates a fetched String cannot corrupt the store.
    #
    # Hot path is lock-free (RCU): see the class header. Expired entries
    # re-check under the shard Mutex before delete so a write that lands
    # between the first observation and eviction is not discarded.
    def read_str(key)
      k = key.to_s
      s = shard_of(k)
      entry = @shards[s][k]
      return nil if entry.nil?
      encoded = expired?(entry) ? live_encoded_after_expiry_recheck(s, k) : entry[0]
      encoded.is_a?(String) ? encoded : nil
    end

    def write_str(key, value, ttl)
      # Dup into the store, then freeze that copy. The caller's
      # accumulator stays mutable; the stored fragment is shared
      # across hits without a read-side dup.
      s_val = (value.is_a?(String) ? value.dup : value.to_s).freeze
      expires_at = ttl.to_i > 0 ? monotonic_now + ttl.to_i : nil
      k = key.to_s
      shard = shard_of(k)
      @mutexes[shard].synchronize { @shards[shard][k] = [s_val, expires_at] }
      s_val
    end

    # The counter behind `rate_limit` (`ActionController::RateLimiter`),
    # the third method of the typed seam: the first increment in a
    # window writes the entry with `ttl` to live and every later one
    # keeps that expiry, as MemoryStore#increment does. Under the
    # shard Mutex, read-modify-write, so two Puma threads counting the
    # same key do not both see the old value.
    def increment_str(key, ttl)
      k = key.to_s
      s = shard_of(k)
      @mutexes[s].synchronize do
        entry = @shards[s][k]
        if entry && !expired?(entry) && entry[0].is_a?(String)
          n = entry[0].to_i + 1
          @shards[s][k] = [n.to_s.freeze, entry[1]]
          n
        else
          @shards[s][k] = ["1".freeze, ttl.to_i > 0 ? monotonic_now + ttl.to_i : nil]
          1
        end
      end
    end

    def read(key)
      k = key.to_s
      s = shard_of(k)
      entry = @shards[s][k]
      return nil if entry.nil?
      encoded = expired?(entry) ? live_encoded_after_expiry_recheck(s, k) : entry[0]
      encoded.nil? ? nil : decode(encoded)
    end

    def write(key, value, opts = {})
      expires_at = nil
      ttl = opts[:expires_in]
      expires_at = monotonic_now + ttl.to_i if ttl
      # Freeze the stored String so `read_str` can hand it back without
      # a copy. `read` still dups (decode), so an untyped caller that
      # mutates what it fetched cannot corrupt the store — the same
      # contract `write_str` already keeps.
      encoded = value.is_a?(String) ? value.dup.freeze : [Marshal.dump(value)]
      k = key.to_s
      s = shard_of(k)
      @mutexes[s].synchronize { @shards[s][k] = [encoded, expires_at] }
      value
    end

    def delete(key)
      k = key.to_s
      s = shard_of(k)
      @mutexes[s].synchronize { @shards[s].delete(k) }
      nil
    end

    def exist?(key)
      !read(key).nil?
    end

    def clear
      s = 0
      while s < SHARD_COUNT
        @mutexes[s].synchronize { @shards[s].clear }
        s += 1
      end
    end

    private

    # Same tail-byte mix as Spinel's `Rails::Cache#shard_of`: fragment
    # keys share a long `views/…` prefix and differ in the id/timestamp
    # at the end; hashing the head would pin a whole room on one shard.
    def shard_of(k)
      n = k.bytesize
      return 0 if n == 0
      ((k.getbyte(n - 1) * 31) + k.getbyte(n / 2)) % SHARD_COUNT
    end

    # Unlocked path already saw expiry. Re-check under the shard lock so
    # a write that landed in between is returned, not discarded as a miss.
    def live_encoded_after_expiry_recheck(s, k)
      encoded = nil
      @mutexes[s].synchronize do
        entry = @shards[s][k]
        if entry && expired?(entry)
          @shards[s].delete(k)
        elsif entry
          encoded = entry[0]
        end
      end
      encoded
    end

    # Entry = [encoded_value, expires_at_or_nil]; a Marshal'd payload is
    # boxed in a 1-elem Array so a cached String and a Marshal String
    # can't be confused.
    def decode(encoded)
      encoded.is_a?(Array) ? Marshal.load(encoded[0]) : encoded.dup
    end

    def expired?(entry)
      at = entry[1]
      !at.nil? && monotonic_now > at
    end

    def monotonic_now
      Process.clock_gettime(Process::CLOCK_MONOTONIC)
    end
  end
end
