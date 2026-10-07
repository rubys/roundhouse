# COMPILED BY SPINEL. Inspect membership only; all mutations use Db's API.
require_relative "db"

class DbConn
  def cache_has?(sql)
    @entries.each { |e| return true if e.sql == sql }
    false
  end

  def cache_size
    @entries.length
  end
end

def read_cached(sql)
  stmt = Db.prepare(sql)
  raise "missing row: " + sql unless Db.step?(stmt)
  value = Db.column_int(stmt, 0)
  Db.finalize(stmt)
  value
end

def fill_cache
  i = 0
  while i < DbConn::CAP
    raise "seed value" unless read_cached("SELECT " + i.to_s) == i
    i += 1
  end
  nil
end

Db.configure(":memory:", pool_size: 1)
cap = DbConn::CAP
mode = ARGV[0]

if mode == "recency"
  Db.with_connection do
    Db.query_cache_end
    fill_cache
    0
  end
  Db.with_connection do
    Db.query_cache_end
    # Repeated hits must not duplicate entries or reset an active cursor.
    3.times { read_cached("SELECT 0") }
    raise "hit grew cache" unless Db.current_conn.cache_size == cap
    read_cached("SELECT " + cap.to_s)
    raise "mid-lease eviction" unless Db.current_conn.cache_size == cap + 1
    0
  end
  raise "hit did not refresh oldest entry" unless Db.current_conn.cache_has?("SELECT 0")
  raise "idle LRU survived" if Db.current_conn.cache_has?("SELECT 1")
elsif mode == "order"
  Db.with_connection do
    Db.query_cache_end
    fill_cache
    # Reverse insertion order: 127 is now LRU, 0 is MRU.
    i = cap - 1
    while i >= 0
      read_cached("SELECT " + i.to_s)
      i -= 1
    end
    read_cached("SELECT " + cap.to_s)
    read_cached("SELECT " + (cap + 1).to_s)
    0
  end
  raise "evicted MRU instead of LRU" unless Db.current_conn.cache_has?("SELECT 0")
  raise "wrong first eviction" if Db.current_conn.cache_has?("SELECT " + (cap - 1).to_s)
  raise "wrong second eviction" if Db.current_conn.cache_has?("SELECT " + (cap - 2).to_s)
elsif mode == "live"
  sql = "SELECT 91 UNION ALL SELECT 92"
  Db.with_connection do
    Db.query_cache_end
    held = Db.prepare(sql)
    raise "first row" unless Db.step?(held) && Db.column_int(held, 0) == 91
    fill_cache
    raise "mid-lease eviction" unless Db.current_conn.cache_size == cap + 1
    raise "in-use entry lost" unless Db.current_conn.cache_has?(sql)
    # Refreshing a busy hit moves the cached entry without lending its
    # live cursor to another reader. That reader owns a transient instead.
    nested = Db.prepare(sql)
    raise "promotion shared active statement" if held == nested
    raise "nested first row" unless Db.step?(nested) && Db.column_int(nested, 0) == 91
    Db.finalize(nested)
    raise "promotion reset cursor" unless Db.step?(held) && Db.column_int(held, 0) == 92
    raise "cursor repeated" if Db.step?(held)
    Db.finalize(held)
    0
  end
  raise "promoted cursor evicted" unless Db.current_conn.cache_has?(sql)
  raise "wrong idle eviction" if Db.current_conn.cache_has?("SELECT 0")
elsif mode == "misses"
  # SQL that inlines its values misses on almost every read, and the cache
  # grows until lease end. A miss that searched the cache made the lease
  # quadratic: its last misses each compared every entry before them.
  # Same-length SQL with a long shared prefix, like a per-value read.
  pad = "x" * 100
  n = 12_000
  block = 500
  first_ms = 0.0
  last_ms = 0.0
  Db.with_connection do
    Db.query_cache_end
    i = 0
    while i < n
      timed = i < block || i >= n - block
      t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      stmt = Db.prepare("SELECT '" + pad + "' AS p, " + (100_000 + i).to_s + " AS v")
      raise "missing row" unless Db.step?(stmt) && Db.column_int(stmt, 1) == 100_000 + i
      Db.finalize(stmt)
      ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000.0
      if timed
        if i < block
          first_ms += ms
        else
          last_ms += ms
        end
      end
      i += 1
    end
    raise "misses not cached" unless Db.current_conn.cache_size == n
    0
  end
  puts "first " + block.to_s + " misses " + first_ms.round(1).to_s + " ms, last " + last_ms.round(1).to_s + " ms"
  raise "a miss scans the cache" if last_ms > first_ms * 5.0 + 2.0
  raise "trim lost the newest entry" unless Db.current_conn.cache_has?("SELECT '" + pad + "' AS p, " + (100_000 + n - 1).to_s + " AS v")
  # A trimmed SQL is prepared again and cached again, not handed back.
  Db.with_connection do
    Db.query_cache_end
    stmt = Db.prepare("SELECT '" + pad + "' AS p, 100000 AS v")
    raise "re-prepare after trim" unless Db.step?(stmt) && Db.column_int(stmt, 1) == 100_000
    Db.finalize(stmt)
    0
  end
  raise "trimmed SQL not cached again" unless Db.current_conn.cache_has?("SELECT '" + pad + "' AS p, 100000 AS v")
else
  raise "unknown case"
end

raise "cache not bounded at lease exit" unless Db.current_conn.cache_size == cap
Db.close
puts "cache " + mode + " passed"
