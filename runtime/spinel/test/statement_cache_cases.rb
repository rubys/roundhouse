# Shared by the emitted runtime suite and tests/param_binds.rs, which runs
# this contract on CRuby and compiles it with Spinel. No app fixture needed.
class StatementCacheTest
  def expect_row(label, stmt, expected)
    raise label + ": cursor ended early" if !Db.step?(stmt)
    actual = Db.column_int(stmt, 0)
    raise label + ": expected " + expected.to_s + ", got " + actual.to_s if actual != expected
  end

  def test_nested_identical_sql
    Db.with_connection do
      Db.query_cache_begin
      sql = "SELECT column1 FROM (VALUES (1), (2), (3)) ORDER BY column1"
      outer = Db.prepare(sql)
      expect_row("identical outer first", outer, 1)
      inner = Db.prepare(sql)
      expect_row("identical inner first", inner, 1)
      expect_row("identical inner second", inner, 2)
      Db.finalize(inner)
      expect_row("identical outer resumed", outer, 2)
      expect_row("identical outer last", outer, 3)
      Db.finalize(outer)
      # The nested captures must not mix their prefixes. Also exercises
      # promotion past the first published (partial) capture.
      replay = Db.prepare(sql)
      expect_row("identical replay first", replay, 1)
      expect_row("identical replay second", replay, 2)
      expect_row("identical replay last", replay, 3)
      raise "identical replay has extra rows" if Db.step?(replay)
      Db.finalize(replay)
      Db.query_cache_end
    end
  end

  def test_nested_bound_sql
    Db.with_connection do
      sql = "SELECT column1 FROM (VALUES (1), (2), (3)) WHERE column1 >= ? ORDER BY column1"
      outer = Db.prepare(sql)
      Db.bind_int(outer, 1, 1)
      expect_row("bound outer first", outer, 1)
      inner = Db.prepare(sql)
      Db.bind_int(inner, 1, 3)
      expect_row("bound inner", inner, 3)
      Db.finalize(inner)
      expect_row("bound outer resumed", outer, 2)
      expect_row("bound outer last", outer, 3)
      Db.finalize(outer)
    end
  end

  def test_checkout_before_step
    Db.with_connection do
      # sqlite3_stmt_busy is false until step; ownership starts at prepare.
      outer = Db.prepare("SELECT ? AS held_bind")
      Db.bind_int(outer, 1, 11)
      inner = Db.prepare("SELECT ? AS held_bind")
      Db.bind_int(inner, 1, 29)
      expect_row("unstepped inner", inner, 29)
      Db.finalize(inner)
      expect_row("unstepped outer", outer, 11)
      Db.finalize(outer)
    end
  end

  def test_raise_then_reuse
    Db.with_connection do
      sql = "SELECT column1, COALESCE(?, -99) FROM (VALUES (1), (2), (3)) WHERE column1 >= ?"
      begin
        abandoned = Db.prepare(sql)
        Db.bind_int(abandoned, 1, 73)
        Db.bind_int(abandoned, 2, 1)
        expect_row("raise first", abandoned, 1)
        raise "hydration failed"
      rescue RuntimeError => e
        raise e if e.message != "hydration failed"
      end
      # A rescued exception is indistinguishable from an intentionally
      # paused cursor: it still owns its statement until the lease ends.
      reused = Db.prepare(sql)
      Db.bind_int(reused, 2, 2)
      expect_row("reuse after raise", reused, 2)
      raise "reuse retained a stale bind" if Db.column_int(reused, 1) != -99
      Db.finalize(reused)
    end
  end

  def test_checkout_until_finalize
    Db.with_connection do
      outer = Db.prepare("SELECT ? AS done_but_owned")
      Db.bind_int(outer, 1, 17)
      expect_row("done outer", outer, 17)
      raise "done outer has extra rows" if Db.step?(outer)
      inner = Db.prepare("SELECT ? AS done_but_owned")
      Db.bind_int(inner, 1, 31)
      expect_row("done inner", inner, 31)
      # Finalize order need not be LIFO, and DONE does not release ownership.
      Db.finalize(outer)
      raise "finalizing outer reset the inner cursor" if Db.step?(inner)
      Db.finalize(inner)
    end
  end

  def test_non_lifo_finalize
    Db.with_connection do
      sql = "SELECT column1 FROM (VALUES (1), (2), (3)) WHERE column1 >= ?"
      outer = Db.prepare(sql)
      Db.bind_int(outer, 1, 1)
      inner = Db.prepare(sql)
      Db.bind_int(inner, 1, 2)
      expect_row("non-LIFO outer", outer, 1)
      expect_row("non-LIFO inner", inner, 2)
      Db.finalize(outer)
      reused = Db.prepare(sql)
      Db.bind_int(reused, 1, 1)
      expect_row("non-LIFO reused first", reused, 1)
      Db.finalize(inner)
      expect_row("non-LIFO reused resumed", reused, 2)
      Db.finalize(reused)
    end
  end

  def test_lease_cleans_abandoned_cursors
    Db.exec("CREATE TABLE cache_lease_rows (id INTEGER)")
    Db.exec("INSERT INTO cache_lease_rows VALUES (1), (2), (3)")
    begin
      Db.with_connection do
        sql = "SELECT id FROM cache_lease_rows WHERE id >= ?"
        outer = Db.prepare(sql)
        Db.bind_int(outer, 1, 1)
        expect_row("abandoned cached", outer, 1)
        inner = Db.prepare(sql)
        Db.bind_int(inner, 1, 2)
        expect_row("abandoned transient", inner, 2)
        raise "request failed"
      end
    rescue RuntimeError => e
      raise e if e.message != "request failed"
    end
    # An abandoned cursor, even on another pooled connection, prevents
    # DROP TABLE in the gate's shared in-memory database.
    Db.exec("DROP TABLE cache_lease_rows")
    Db.exec("CREATE TABLE cache_lease_rows (id INTEGER)")
    Db.exec("INSERT INTO cache_lease_rows VALUES (3)")
    Db.with_connection do
      stmt = Db.prepare("SELECT id FROM cache_lease_rows WHERE id >= ?")
      Db.bind_int(stmt, 1, 3)
      expect_row("next lease", stmt, 3)
      Db.finalize(stmt)
    end
    Db.exec("DROP TABLE cache_lease_rows")
  end

  def test_overlapping_replay_promotions
    Db.with_connection do
      Db.query_cache_begin
      sql = "SELECT column1 FROM (VALUES (10), (20), (30)) ORDER BY column1"
      seed = Db.prepare(sql)
      expect_row("replay seed", seed, 10)
      Db.finalize(seed)
      outer = Db.prepare(sql)
      expect_row("replay outer prefix", outer, 10)
      expect_row("replay outer promoted", outer, 20)
      inner = Db.prepare(sql)
      expect_row("replay inner prefix", inner, 10)
      expect_row("replay inner promoted", inner, 20)
      Db.finalize(inner)
      expect_row("replay outer resumed", outer, 30)
      Db.finalize(outer)
      Db.query_cache_end
    end
  end

  def test_lease_cleans_abandoned_replay
    Db.exec("CREATE TABLE cache_replay_rows (id INTEGER)")
    Db.exec("INSERT INTO cache_replay_rows VALUES (1), (2), (3)")
    begin
      Db.with_connection do
        Db.query_cache_begin
        sql = "SELECT id FROM cache_replay_rows ORDER BY id"
        seed = Db.prepare(sql)
        expect_row("abandoned replay seed", seed, 1)
        Db.finalize(seed)
        outer = Db.prepare(sql)
        expect_row("abandoned replay prefix", outer, 1)
        expect_row("abandoned replay promotion", outer, 2)
        inner = Db.prepare(sql)
        expect_row("abandoned inner replay prefix", inner, 1)
        expect_row("abandoned inner replay promotion", inner, 2)
        raise "replay failed"
      end
    rescue RuntimeError => e
      raise e if e.message != "replay failed"
    end
    Db.query_cache_end
    Db.exec("DROP TABLE cache_replay_rows")
  end

  def read_bound(sql, value, expected)
    stmt = Db.prepare(sql)
    Db.bind_int(stmt, 1, value)
    expect_row("bound scalar", stmt, expected)
    raise "scalar has extra rows" if Db.step?(stmt)
    Db.finalize(stmt)
  end

  def test_uncached_bind_and_step_errors
    Db.with_connection do
      Db.query_cache_end
      failed = false
      stmt = Db.prepare("SELECT column1 FROM (VALUES (10), (11), (20)) WHERE column1 >= ?")
      Db.bind_int(stmt, 1, 10)
      expect_row("real cursor before misuse", stmt, 10)
      begin
        Db.bind_int(stmt, 1, 20)
      rescue StandardError
        failed = true
      end
      raise "real SQLITE_MISUSE was ignored" if !failed
      stmt = Db.prepare("SELECT column1 FROM (VALUES (10), (11), (20)) WHERE column1 >= ?")
      Db.bind_int(stmt, 1, 20)
      expect_row("next owner after real misuse", stmt, 20)
      Db.finalize(stmt)
      failed = false
      stmt = Db.prepare("SELECT abs(-9223372036854775808)")
      begin
        Db.step?(stmt)
      rescue StandardError
        failed = true
      end
      raise "uncached step error became EOF" if !failed
      read_bound("SELECT ? AS usable_after_error", 31, 31)
    end
  end

  def test_finalize_after_bind_error
    Db.with_connection do
      sql = "SELECT COALESCE(?, -99) AS bind_error_ownership"
      old = Db.prepare(sql)
      current = nil
      begin
        failed = false
        begin
          Db.bind_int(old, 2, 1)
        rescue StandardError
          failed = true
        end
        raise "invalid bind was ignored" if !failed
        current = Db.prepare(sql)
        Db.bind_int(current, 1, 73)
      ensure
        # The caller still owns old after rescuing its driver error.
        # Its raw pointer must not identify the new checkout as well.
        Db.finalize(old)
      end
      expect_row("finalize after bind error preserves new reader", current, 73)
      Db.finalize(current)
    end
  end

  def test_finalize_after_step_error
    Db.with_connection do
      sql = "SELECT CASE WHEN ? = 0 THEN abs(-9223372036854775808) ELSE COALESCE(?, -99) END"
      old = Db.prepare(sql)
      current = nil
      begin
        Db.bind_int(old, 1, 0)
        failed = false
        begin
          Db.step?(old)
        rescue StandardError
          failed = true
        end
        raise "integer overflow was ignored" if !failed
        current = Db.prepare(sql)
        Db.bind_int(current, 1, 1)
        Db.bind_int(current, 2, 73)
      ensure
        Db.finalize(old)
      end
      expect_row("finalize after step error preserves new reader", current, 73)
      Db.finalize(current)
    end
  end

  def run
    test_uncached_bind_and_step_errors
    test_finalize_after_bind_error
    test_finalize_after_step_error
    test_nested_identical_sql
    test_nested_bound_sql
    test_checkout_before_step
    test_checkout_until_finalize
    test_non_lifo_finalize
    test_raise_then_reuse
    test_lease_cleans_abandoned_cursors
    test_overlapping_replay_promotions
    test_lease_cleans_abandoned_replay
    nil
  end
end
