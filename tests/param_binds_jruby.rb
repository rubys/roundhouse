# Direct JDBC contract, including the shared varying-id/concurrency gate:
#   jruby tests/param_binds_jruby.rb
# Requires JRuby 10+ and the jdbc-sqlite3 gem.
raise "JRuby required" unless RUBY_ENGINE == "jruby"

require_relative "../runtime/ruby/active_record/errors"
require_relative "../runtime/spinel/db_jruby"
require_relative "../runtime/spinel/test/statement_cache_cases"

# This contract holds four simultaneous leases, even under a caller's pool override.
ENV["DATABASE_POOL_SIZE"] = "4"
Db.configure("file:jruby_param_binds?mode=memory&cache=shared", pool_size: 4)
begin
  StatementCacheTest.new.run
  puts "jruby: 12 statement cache ownership and error tests passed"
  require_relative "param_binds_runtime"
  require_relative "param_binds_nil"

  inline_bound_string("NUL UTF-8", "a\0b")
  inline_bound_string("NUL binary", "a\0b".b)
  abandoned = nil
  owner = nil
  begin
    Db.with_connection do
      owner = Db.current_dbh
      stmt = Db.prepare_uncached("SELECT 1 WHERE 1 IN (1, 2)")
      abandoned = stmt.pstmt
      raise "intentional uncached unwind"
    end
  rescue RuntimeError => error
    raise unless error.message == "intentional uncached unwind"
  end
  raise "uncached JDBC statement leaked on unwind" unless abandoned.is_closed
  raise "uncached JDBC ownership leaked" unless owner.open_statements.empty?
  puts "jruby: NUL writer parity and uncached lease cleanup passed"

  Db.with_connection do
    Db.query_cache_begin
    begin
      sql = "SELECT CAST(? AS TEXT) AS integer_text, ? AS flag, typeof(?) AS flag_type"
      cached = nil
      [2**40 + 123, -(2**40 + 123), 2**63 - 1, -(2**63)].each_with_index do |value, index|
        flag = index.even?
        stmt = Db.prepare(sql)
        raise "prepare executed before binding" if stmt.executed
        raise "cached PreparedStatement was not reused" if cached && !cached.equal?(stmt.pstmt)
        cached = stmt.pstmt
        Db.bind_int(stmt, 1, value)
        Db.bind_bool(stmt, 2, flag)
        Db.bind_bool(stmt, 3, flag)
        raise "binding executed the query" if stmt.executed
        # Dynamic row readers ask for metadata before fetching their first
        # row. This must execute once, after all the parameters are bound.
        expect_int("metadata column count", 3, Db.column_count(stmt))
        expect_text("metadata column name", "integer_text", Db.column_name(stmt, 0))
        rs = stmt.rs
        raise "missing integer/bool row" unless Db.step?(stmt)
        raise "metadata query executed twice" unless rs.equal?(stmt.rs)
        # column_int currently reads JDBC int, so observe the entire bound
        # 64-bit value as text rather than testing that separate read API.
        expect_text("64-bit integer bind", value.to_s, Db.column_text(stmt, 0))
        expect_int("boolean value", flag ? 1 : 0, Db.column_int(stmt, 1))
        expect_text("boolean storage type", "integer", Db.column_text(stmt, 2))
        raise "unexpected second row" if Db.step?(stmt)
        Db.finalize(stmt)
        raise "cached statement closed on finalize" if cached.is_closed
      end

      # A cached statement retains its parameters in JDBC unless cleared.
      # Check release and checkout independently, including finalize without
      # any execution, so a future missed bind cannot replay an old value.
      sql = "SELECT ? IS NULL AS cleared"
      stmt = Db.prepare(sql)
      cached = stmt.pstmt
      Db.bind_int(stmt, 1, 91)
      Db.finalize(stmt)
      rs = cached.execute_query
      raise "missing clear-on-finalize row" unless rs.next
      expect_int("finalize clears parameters", 1, rs.get_int(1))
      rs.close
      stmt = Db.prepare(sql)
      raise "clean idle reuse used a new statement" unless cached.equal?(stmt.pstmt)
      raise "missing clear-on-checkout row" unless Db.step?(stmt)
      expect_int("idle reuse keeps cleared parameters", 1, Db.column_int(stmt, 0))
      Db.finalize(stmt)

      # Partial stepping must release the ResultSet for the next bound
      # execution of the same cached SQL, even if the previous one had rows.
      sql = "SELECT id FROM bind_rows WHERE id >= ? ORDER BY id"
      [1, 17, 4, 31, 2].each do |id|
        stmt = Db.prepare(sql)
        Db.bind_int(stmt, 1, id)
        raise "missing partial cursor row" unless Db.step?(stmt)
        expect_int("partial cursor reset", id, Db.column_int(stmt, 0))
        rs = stmt.rs
        Db.finalize(stmt)
        raise "ResultSet left open on finalize" unless rs.is_closed
      end

      # Once the cache is full, bound statements remain transient and must
      # close even if the caller finalizes before the first step.
      Db::STMT_CACHE_CAP.times do |i|
        stmt = Db.prepare("SELECT #{i} AS cache_filler")
        Db.finalize(stmt)
      end
      stmt = Db.prepare("SELECT ? AS transient_value")
      raise "expected a transient statement" if stmt.cached
      Db.bind_text(stmt, 1, "discarded")
      transient = stmt.pstmt
      Db.finalize(stmt)
      raise "transient statement left open" unless transient.is_closed
      stmt = Db.prepare("SELECT ? AS transient_value")
      Db.bind_text(stmt, 1, "fresh 雪")
      raise "missing transient value" unless Db.step?(stmt)
      expect_text("transient bind", "fresh 雪", Db.column_text(stmt, 0))
      Db.finalize(stmt)
    ensure
      Db.query_cache_end
    end
  end
  puts "jruby: 64-bit integers, booleans, lazy metadata, clearing, reuse and transient closure passed"
ensure
  Db.close
end

ENV["DATABASE_POOL_SIZE"] = "1"
require_relative "support/jdbc_cleanup_failures"
