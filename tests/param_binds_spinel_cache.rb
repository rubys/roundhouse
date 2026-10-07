# Native ownership checks: count actual SQLite statements, including ones
# accidentally lost from the cache. This FFI hook is test-only.
module SQL
  ffi_func :sqlite3_next_stmt, [:ptr, :ptr], :ptr
end

def native_statement_count(conn)
  count = 0
  ptr = SQL.sqlite3_next_stmt(conn.dbh, nil)
  while !ptr.nil?
    count += 1
    ptr = SQL.sqlite3_next_stmt(conn.dbh, ptr)
  end
  count
end

# Snapshot BEGIN/COMMIT are internal cache users too. They must release
# each checkout immediately, rather than accumulating busy-hit siblings.
Db.with_connection do
  conn = Db.current_conn
  raise "snapshot BEGIN failed" if !conn.run_cached("BEGIN")
  raise "snapshot COMMIT failed" if !conn.run_cached("COMMIT")
  before = native_statement_count(conn)
  i = 0
  while i < 12
    raise "snapshot BEGIN reuse failed" if !conn.run_cached("BEGIN")
    raise "snapshot COMMIT reuse failed" if !conn.run_cached("COMMIT")
    i += 1
  end
  expect_int("snapshot commands release their checkouts", before, native_statement_count(conn))
  raise "internal cached error was ignored" if conn.run_cached("SELECT abs(-9223372036854775808)")
  expect_int("failed internal command releases ownership", before, native_statement_count(conn))
end
puts "runtime: native snapshot statement ownership passed"

# A write can invalidate a live replay's saved prefix. If its replacement
# query fails while skipping that prefix, ensure-finalize still owns it.
Db.with_connection do
  conn = Db.current_conn
  Db.exec("CREATE TABLE replay_error_rows (id INTEGER PRIMARY KEY, value INTEGER)")
  Db.exec("INSERT INTO replay_error_rows VALUES (1, 1), (2, 2)")
  sql = "SELECT abs(value) FROM replay_error_rows ORDER BY id"
  seed = Db.prepare(sql)
  raise "missing replay error seed" if !Db.step?(seed)
  expect_int("replay error seed", 1, Db.column_int(seed, 0))
  Db.finalize(seed)
  replay = Db.prepare(sql)
  raise "missing saved replay prefix" if !replay.is_a?(Integer) || !Db.step?(replay)
  expect_int("saved replay prefix", 1, Db.column_int(replay, 0))
  Db.exec("UPDATE replay_error_rows SET value = -9223372036854775808 WHERE id = 1")
  failed = false
  begin
    Db.step?(replay)
  rescue RuntimeError => error
    raise error if !error.message.include?("integer overflow")
    failed = true
  ensure
    Db.finalize(replay)
  end
  raise "replayed prefix overflow was ignored" if !failed
  before = native_statement_count(conn)
  current = Db.prepare(sql)
  expect_int("failed replay finalizer releases cached ownership", before, native_statement_count(conn))
  Db.finalize(current)
  Db.exec("DROP TABLE replay_error_rows")
end
puts "runtime: failed replay prefix finalizer releases cached ownership passed"

Db.with_connection do
  conn = Db.current_conn
  outer = Db.prepare("SELECT ? AS transient_ownership")
  Db.bind_int(outer, 1, 71)
  before = native_statement_count(conn)
  inner = Db.prepare("SELECT ? AS transient_ownership")
  expect_int("busy hit prepares a transient", before + 1, native_statement_count(conn))
  Db.finalize(inner)
  expect_int("transient is really finalized", before, native_statement_count(conn))
  Db.finalize(outer)
end

# A latent SQLite execution error makes reset report a failure. The lease
# must still release every sibling, destroy the failed cached statement,
# and preserve its request error. Call FFI step directly so this covers
# release independently of the checked-step production path.
cleanup_conn = nil
before_cleanup = 0
begin
  Db.with_connection do
    cleanup_conn = Db.current_conn
    before_cleanup = native_statement_count(cleanup_conn)
    bad = Db.prepare("SELECT abs(-9223372036854775808) AS cleanup_native_failure")
    rc = SQL.sqlite3_step(bad)
    raise "missing native reset failure" if rc == SQL::ROW || rc == SQL::DONE
    a = Db.prepare("SELECT column1 FROM (VALUES (1), (2)) AS cleanup_native_sibling")
    b = Db.prepare("SELECT column1 FROM (VALUES (1), (2)) AS cleanup_native_sibling")
    Db.step?(a)
    Db.step?(b)
    raise "native request failed before cleanup"
  end
rescue RuntimeError => e
  raise e if e.message != "native request failed before cleanup"
end
expect_int("native failed cache entry and transient sibling were finalized", before_cleanup + 1, native_statement_count(cleanup_conn))
Db.with_connection do
  stmt = Db.prepare("SELECT 29 AS cleanup_native_recovery")
  raise "native cleanup made the lease unusable" if !Db.step?(stmt)
  expect_int("native cleanup recovery", 29, Db.column_int(stmt, 0))
  Db.finalize(stmt)
end
puts "runtime: native reset failure drains siblings and preserves request errors passed"

# trim! must preserve a cursor even if explicitly called mid-lease.
Db.with_connection do
  conn = Db.current_conn
  outer = Db.prepare("SELECT column1 FROM (VALUES (1), (2), (3)) WHERE column1 >= ?")
  Db.bind_int(outer, 1, 1)
  raise "missing trim seed" if !Db.step?(outer)
  i = 0
  while i < 140
    stmt = Db.prepare("SELECT " + i.to_s + " AS trim_pressure")
    Db.finalize(stmt)
    i += 1
  end
  conn.trim!
  raise "trim closed a live cursor" if !Db.step?(outer)
  expect_int("trim preserves cursor", 2, Db.column_int(outer, 0))
  Db.finalize(outer)
end

# Exceptional leases must also trim; cleanup includes busy-hit transients.
begin
  Db.with_connection do
    i = 0
    while i < 140
      stmt = Db.prepare("SELECT " + i.to_s + " AS exception_pressure")
      Db.finalize(stmt)
      i += 1
    end
    abandoned = Db.prepare("SELECT ? AS abandoned_pressure")
    nested = Db.prepare("SELECT ? AS abandoned_pressure")
    raise "pressure failed"
  end
rescue RuntimeError => e
  raise e if e.message != "pressure failed"
end
Db.with_connection do
  conn = Db.current_conn
  raise "exception bypassed trim" if native_statement_count(conn) > DbConn::CAP
  outer = Db.prepare("SELECT ? AS shutdown_ownership")
  inner = Db.prepare("SELECT ? AS shutdown_ownership")
  conn.finalize_all
  expect_int("shutdown finalizes cached and transient statements", 0, native_statement_count(conn))
end
puts "runtime: native transient release, trimming and shutdown passed"

# Shutdown must finish across every connection and shard before reporting a
# release error. Real SQLite step errors leave reset failing at shutdown.
module SQL
  ffi_func :sqlite3_memory_used, [], :long
end

class DbConn
  def shutdown_cache_size
    @entries.length
  end

  def shutdown_open_size
    @open.length
  end
end

module Db
  def self.shutdown_pools
    @pools
  end
end

Db.close
before_shutdown = SQL.sqlite3_memory_used
ENV["DATABASE_POOL_SIZE"] = "12"
shutdown_path = "file:shutdown_release_failure?mode=memory&cache=shared"
Db.configure(shutdown_path, pool_size: 12)
Db.exec("CREATE TABLE shutdown_rows (id INTEGER PRIMARY KEY)")
Db.exec("INSERT INTO shutdown_rows VALUES (1)")
pools = Db.shutdown_pools
expect_int("shutdown uses multiple shards", 3, pools.length)
connections = []
p = 0
while p < pools.length
  i = 0
  while i < 4
    conn = pools[p].conn(i)
    connections.push(conn)
    idle = conn.prepare_cached("SELECT 37 AS shutdown_idle")
    conn.release(idle)
    held = conn.prepare_cached("SELECT 41 AS shutdown_held")
    transient = conn.prepare_cached("SELECT 41 AS shutdown_held")
    expect_int("shutdown cached reader", SQL::ROW, SQL.sqlite3_step(held))
    expect_int("shutdown transient reader", SQL::ROW, SQL.sqlite3_step(transient))
    if p == 0 && i == 0
      bad = conn.prepare_cached("SELECT abs(-9223372036854775808)")
      expect_int("shutdown first release failure", 1, SQL.sqlite3_step(bad))
    elsif i == 0 || (p == 0 && i == 1)
      bad = conn.prepare_cached("INSERT INTO shutdown_rows VALUES (1)")
      expect_int("shutdown later release failure", 19, SQL.sqlite3_step(bad))
    end
    i += 1
  end
  p += 1
end
shutdown_error = nil
begin
  Db.close
rescue RuntimeError => e
  shutdown_error = e
end
connections.each do |conn|
  expect_int("shutdown clears every cache", 0, conn.shutdown_cache_size)
  expect_int("shutdown drains every checkout", 0, conn.shutdown_open_size)
end
# This accounts for actual native statements and handles, without reading
# freed pointers. A leaked connection also keeps the shared memory DB alive.
expect_int("shutdown frees every SQLite resource", before_shutdown, SQL.sqlite3_memory_used)
raise "shutdown retained pools" if !Db.shutdown_pools.nil?
raise "shutdown lost the first release error" if shutdown_error.nil? || !shutdown_error.message.start_with?("Db.release failed (1):")
Db.close
ENV["DATABASE_POOL_SIZE"] = "1"
Db.configure(shutdown_path, pool_size: 1)
probe = Db.prepare("SELECT COUNT(*) FROM sqlite_master WHERE name = 'shutdown_rows'")
raise "shutdown database probe failed" if !Db.step?(probe)
expect_int("shutdown closes every shared-memory connection", 0, Db.column_int(probe, 0))
Db.finalize(probe)
puts "runtime: native shutdown drains all shards and preserves the first release error passed"
