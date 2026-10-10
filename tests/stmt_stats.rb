require_relative "db"

# The flag must remain the value captured at boot even if an application
# changes its environment later. File destinations are captured too.
ENV["RH_STMT_STATS"] = DbConn::STMT_STATS ? "0" : "1"

def point(sql)
  stmt = Db.prepare(sql)
  raise "missing point" unless Db.step?(stmt)
  value = Db.column_int(stmt, 0)
  Db.finalize(stmt)
  value
end

def bound(value, nullable = false, transient = false)
  stmt = transient ? Db.prepare_uncached("SELECT COALESCE(?, -99)") : Db.prepare("SELECT COALESCE(?, -99)")
  if nullable
    Db.bind_int_opt(stmt, 1, nil)
  else
    Db.bind_int(stmt, 1, value)
  end
  raise "missing bound point" unless Db.step?(stmt)
  got = Db.column_int(stmt, 0)
  Db.finalize(stmt)
  got
end

Db.configure(":memory:", pool_size: 1)
Db.dump_stmt_stats("empty")
raise "outside lease" unless point("SELECT 1") == 1
Db.dump_stmt_stats("plain")

Db.with_connection do
  raise "first point" unless point("SELECT 1") == 1
  raise "inline replay" unless point("SELECT 1") == 1
  raise "first bind" unless bound(7) == 7
  raise "repeated bind" unless bound(7) == 7
  raise "varying bind" unless bound(8) == 8
  raise "NULL bind" unless bound(0, true) == -99
  raise "repeated NULL bind" unless bound(0, true) == -99
end
Db.dump_stmt_stats("bound")

Db.with_connection do
  first = Db.prepare_uncached("SELECT 9 UNION ALL SELECT 10")
  raise "transient first" unless Db.step?(first) && Db.column_int(first, 0) == 9
  Db.finalize(first)
  replay = Db.prepare_uncached("SELECT 9 UNION ALL SELECT 10")
  raise "transient replay" unless Db.step?(replay) && Db.column_int(replay, 0) == 9
  # A native prepare occurs only when the partial replay needs more rows.
  raise "transient promotion" unless Db.step?(replay) && Db.column_int(replay, 0) == 10
  raise "transient eof" if Db.step?(replay)
  Db.finalize(replay)
  raise "transient bound" unless bound(12, false, true) == 12
  raise "repeated transient bind" unless bound(12, false, true) == 12
end
Db.dump_stmt_stats("transient")

# An idle pool snapshot still includes an explicitly-owned transient
# cursor opened by a script outside a request lease.
live = Db.prepare_uncached("SELECT 314")
raise "live transient" unless Db.step?(live) && Db.column_int(live, 0) == 314
Db.dump_stmt_stats("live")
Db.finalize(live)
Db.dump_stmt_stats("released")

# Checked step consumes the driver's error with a reset but retains ownership.
# Lease cleanup can then reuse this statement; it is not a finalization.
Db.with_connection do
  bad = Db.prepare("SELECT abs(-9223372036854775808)")
  begin
    Db.step?(bad)
    raise "step should fail"
  rescue RuntimeError => e
    raise e unless e.message.include?("Db.step failed") && e.message.include?("integer overflow")
  end
end
Db.dump_stmt_stats("failed")

# Leave SQLite's error pending to exercise a genuinely failed cached reset.
# The direct FFI call is fault injection, not the normal checked-step path.
Db.with_connection do
  conn = Db.current_conn
  bad = conn.prepare_cached("SELECT abs(-9223372036854775808) /* pending reset error */")
  raise "raw step should fail" if SQL.sqlite3_step(bad) == SQL::ROW
  begin
    conn.release(bad)
    raise "release should fail"
  rescue RuntimeError => e
    raise e unless e.message.include?("Db.release failed")
  end
end
Db.dump_stmt_stats("reset_failed")

Db.with_connection do
  begin
    Db.prepare("SELECT missing_column")
    raise "prepare should fail"
  rescue RuntimeError => e
    raise e unless e.message.include?("Db.prepare failed")
  end
  i = 0
  while i < 130
    raise "trim read" unless point("SELECT " + i.to_s + " + 0") == i
    i += 1
  end
  # A prepare without a step must not count as an execution.
  Db.finalize(Db.prepare("SELECT 999"))
end
Db.dump_stmt_stats("trimmed")
Db.close
puts "stmt stats probe passed"
