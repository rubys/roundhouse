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
# workers=0), matching MemoryStore's own scope. Thread-safety via a
# single Mutex, as MemoryStore does.
#
# CRuby-only (overlay, not runtime/ruby): Marshal/Mutex/Time-based
# eviction are exactly the is_a?-dispatching dynamic shapes the shared
# runtime's typing bar excludes; other targets keep the no-op until
# their lobsters turn.
module Rails
  def self.cache
    @cache_store ||= MemoryStore.new
  end

  class MemoryStore
    def initialize
      @data  = {}
      @mutex = Mutex.new
    end

    def fetch(key, opts = {})
      k = key.to_s
      @mutex.synchronize do
        entry = @data[k]
        return decode(entry[0]) if entry && !expired?(entry)
      end
      value = yield
      write(key, value, opts)
      value
    end

    def read(key)
      @mutex.synchronize do
        entry = @data[key.to_s]
        return nil if entry.nil?
        if expired?(entry)
          @data.delete(key.to_s)
          return nil
        end
        decode(entry[0])
      end
    end

    def write(key, value, opts = {})
      expires_at = nil
      ttl = opts[:expires_in]
      expires_at = monotonic_now + ttl.to_i if ttl
      encoded = value.is_a?(String) ? value.dup : [Marshal.dump(value)]
      @mutex.synchronize { @data[key.to_s] = [encoded, expires_at] }
      value
    end

    def delete(key)
      @mutex.synchronize { @data.delete(key.to_s) }
      nil
    end

    def exist?(key)
      !read(key).nil?
    end

    def clear
      @mutex.synchronize { @data.clear }
    end

    private

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
