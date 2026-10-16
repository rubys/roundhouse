# The Spinel shim's SQLite concurrency policy (runtime/spinel/db.rb), compiled
# by spinel and run against a file database: the read snapshot, the write
# permit and its bounded wait, the abandoned-transaction cleanup, and the
# background checkpointer. The CRuby shim's checks are the Ruby scripts in
# tests/db_sqlite_concurrency.rs; this is the same list in the subset spinel
# compiles (no instance_variable_get, no Queue, every lease block ends in a
# value). Driven by that file's `spinel_shim_policy` test.

require_relative "db"
module ActiveRecord
  class ActiveRecordError < StandardError
  end
  class StatementInvalid < ActiveRecordError
  end
  class WrappedDatabaseException < StatementInvalid
  end
  class RecordNotUnique < WrappedDatabaseException
  end
  class InvalidForeignKey < WrappedDatabaseException
  end
  class ValueTooLong < StatementInvalid
  end
  class RangeError < StatementInvalid
  end
  class NotNullViolation < StatementInvalid
  end
  class CheckViolation < StatementInvalid
  end
  class ExclusionViolation < StatementInvalid
  end
  class TransactionRollbackError < StatementInvalid
  end
  class SerializationFailure < TransactionRollbackError
  end
  class Deadlocked < TransactionRollbackError
  end
  class QueryAborted < StatementInvalid
  end
  class LockWaitTimeout < StatementInvalid
  end
  class StatementTimeout < QueryAborted
  end
  class QueryCanceled < QueryAborted
  end
end
def check(what, ok)
  if !ok
    $stderr.puts "FAILED: " + what
    exit(1)
  end
  puts "ok: " + what
end
def count(table)
  st = Db.prepare("SELECT COUNT(*) FROM " + table)
  Db.step?(st)
  n = Db.column_int(st, 0)
  Db.finalize(st)
  n
end
path = ARGV[0]
Db.configure(path, pool_size: 4)

# snapshot
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)")
Db.exec("INSERT INTO t VALUES (1)")
Db.with_connection do
  Db.read_snapshot_begin
  check("snapshot is lazy", !Db.current_conn.in_txn?)
  check("first read", count("t") == 1)
  check("snapshot opened by the first read", Db.current_conn.in_txn?)
  w = Thread.new do
    Db.with_connection { Db.exec("INSERT INTO t VALUES (2)") }
    nil
  end
  w.join
  check("other connection's commit is invisible inside the snapshot", count("t") == 1)
  Db.exec("INSERT INTO t VALUES (3)")
  check("the read after a write sees every commit", count("t") == 3)
  Db.read_snapshot_end
  check("snapshot closed at the end", !Db.current_conn.in_txn?)
  true
end

# nesting + app transaction
Db.with_connection do
  Db.read_snapshot_begin
  Db.read_snapshot_begin
  count("t")
  Db.read_snapshot_end
  check("inner end keeps the snapshot", Db.current_conn.in_txn?)
  Db.read_snapshot_end
  check("outer end closes it", !Db.current_conn.in_txn?)
  Db.read_snapshot_begin
  Db.exec("BEGIN")
  Db.exec("INSERT INTO t VALUES (4)")
  check("a read inside the app's transaction", count("t") == 4)
  Db.exec("COMMIT")
  check("the app's COMMIT closed its transaction", !Db.current_conn.in_txn?)
  Db.read_snapshot_end
  true
end

# permit: writers queue, never meet SQLITE_BUSY
Db.exec("CREATE TABLE w (id INTEGER PRIMARY KEY, n INTEGER)")
errors = 0
lock = Mutex.new
threads = []
i = 0
while i < 8
  ti = i
  threads.push(Thread.new do
    Db.with_connection do
      Db.exec("PRAGMA busy_timeout=0")
      j = 0
      while j < 25
        begin
          if j % 2 == 0
            Db.exec("INSERT INTO w (n) VALUES (" + ti.to_s + ")")
          else
            Db.exec("BEGIN")
            Db.exec("INSERT INTO w (n) VALUES (" + ti.to_s + ")")
            sleep(0.0005)
            Db.exec("INSERT INTO w (n) VALUES (" + ti.to_s + ")")
            Db.exec("COMMIT")
          end
        rescue => e
          lock.synchronize { errors += 1 }
        end
        j += 1
      end
      true
    end
    nil
  end)
  i += 1
end
threads.each { |t| t.join }
check("no writer met SQLITE_BUSY (" + errors.to_s + ")", errors == 0)
check("every row landed", count("w") == 8 * (13 + 12 * 2))

# abandoned transaction
begin
  Db.with_connection do
    Db.exec("BEGIN")
    Db.exec("INSERT INTO t VALUES (99)")
    raise "abandon" if count("t") > 0
    nil
  end
rescue => e
end
w2 = Thread.new do
  Db.with_connection { Db.exec("INSERT INTO t VALUES (5)") }
  nil
end
w2.join
check("abandoned insert rolled back, permit freed", count("t") == 5)

# bounded wait: a transaction joining its own writer errors instead of hanging
Db.write_permit_timeout = 0.2
outcome = ""
Db.with_connection do
  Db.exec("BEGIN")
  Db.exec("INSERT INTO t VALUES (6)")
  child = Thread.new do
    Db.with_connection do
      Db.exec("PRAGMA busy_timeout=100")
      begin
        Db.exec("INSERT INTO t VALUES (7)")
        outcome = "wrote"
      rescue => e
        outcome = e.message.include?("(5)") ? "busy" : e.message
      end
      true
    end
    nil
  end
  child.join
  Db.exec("COMMIT")
  true
end
check("inner writer met SQLITE_BUSY (" + outcome + ")", outcome == "busy")
check("outer transaction committed", count("t") == 6)
Db.write_permit_timeout = 5.0

# checkpoints off the request path
Db.exec("CREATE TABLE b (id INTEGER PRIMARY KEY, b BLOB)")
Db.checkpoint_in_background!
before = File.size(path)
Db.with_connection do
  st = Db.prepare("PRAGMA wal_autocheckpoint")
  Db.step?(st)
  v = Db.column_int(st, 0)
  Db.finalize(st)
  check("leased connection no longer autocheckpoints", v == 0)
  k = 0
  while k < 200
    Db.exec("INSERT INTO b (b) VALUES (randomblob(4096))")
    k += 1
  end
  true
end
waited = 0
while File.size(path) <= before + 400000 && waited < 100
  sleep(0.05)
  waited += 1
end
check("background checkpoint copied the log", File.size(path) > before + 400000)
puts "OK"
