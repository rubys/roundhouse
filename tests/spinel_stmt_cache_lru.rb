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
    # Moving a hit must move the same statement, preserving its cursor.
    same = Db.prepare(sql)
    raise "promotion replaced statement" unless held == same
    raise "promotion reset cursor" unless Db.step?(held) && Db.column_int(held, 0) == 92
    raise "cursor repeated" if Db.step?(held)
    Db.finalize(held)
    0
  end
  raise "promoted cursor evicted" unless Db.current_conn.cache_has?(sql)
  raise "wrong idle eviction" if Db.current_conn.cache_has?("SELECT 0")
else
  raise "unknown case"
end

raise "cache not bounded at lease exit" unless Db.current_conn.cache_size == cap
Db.close
puts "cache " + mode + " passed"
