# Primitive Db surface — JRuby variant. Same `module Db` contract as
# `db_cruby.rb` (the CRuby/`sqlite3`-gem shim) and `db.rb` (the spinel
# FFI shim), but backed by JDBC: the `sqlite3` gem is a C extension with
# no JRuby build, so JRuby talks to SQLite through the Xerial
# `sqlite-jdbc` driver (the `jdbc-sqlite3` gem) over `java.sql`.
#
# API (identical to db_cruby.rb — `sqlite_adapter.rb` and the
# lowerer-emitted `_adapter_*` methods are unchanged across all shims):
#
#   Db.configure(path)         — open a database (":memory:" for tests)
#   Db.close                   — close all connections
#   Db.exec(sql)               — run DDL / INSERT / UPDATE / DELETE
#   Db.prepare(sql)            — prepare a SELECT, returns a stmt handle
#   Db.step?(stmt)             — advance, returns true if a row arrived
#   Db.column_int(stmt, i)     — read int column at zero-based index
#   Db.column_text(stmt, i)    — read text column at zero-based index
#   Db.column_count(stmt)      — number of columns in the prepared row
#   Db.column_name(stmt, i)    — name of column at zero-based index
#   Db.finalize(stmt)          — release the prepared stmt
#   Db.last_insert_rowid       — id of the last INSERTed row
#   Db.changes                 — affected-row count of the last statement
#
# THREAD-SAFETY (the one place this diverges from db_cruby.rb): JRuby has
# no GVL, so Puma's worker threads run truly in parallel. db_cruby.rb
# keys statement handles into a single global `@rows` hash with a shared
# `@next_id` counter — safe only under CRuby's GVL. Here the stmt handle
# is instead an opaque `Stmt` wrapper object returned straight from
# `prepare`; callers only ever pass it back to `Db.*` (verified against
# `sqlite_adapter.rb`), so there is no shared mutable handle table to
# race on. The per-connection prepared-statement cache lives on the
# leased `Conn` (one thread at a time via `with_connection`), so it needs
# no lock either — same invariant db_cruby.rb relies on.
#
# JDBC notes: column indices are 1-based (we add 1 to the zero-based
# contract index). `column_count`/`column_name` read the
# PreparedStatement's metadata, which the sqlite-jdbc driver resolves at
# prepare time — `sqlite_adapter.rb` calls `column_count` before the
# first `step?`, so we must not depend on a ResultSet existing yet.

require "jdbc/sqlite3"
Jdbc::SQLite3.load_driver
# NB: connect through `org.sqlite.SQLiteDataSource`, NOT
# `java.sql.DriverManager`. DriverManager lives in the JVM bootstrap
# classloader and can't see the sqlite-jdbc driver that the gem registers
# from JRuby's classloader, so `DriverManager.getConnection` raises "No
# suitable driver found". The SQLiteDataSource instantiates the driver
# directly, sidestepping that visibility gap.
java_import org.sqlite.SQLiteDataSource

module Db
  # Per-connection prepared-statement cache bound (roundhouse#12). Mirrors
  # db_cruby.rb: beyond this many distinct SQL strings on one connection,
  # further statements are transient (closed on finalize) rather than
  # cached — bounds growth when inlined literals key queries per-id.
  STMT_CACHE_CAP = 128

  # A pooled connection plus its prepared-statement cache. The cache is
  # keyed by composed SQL (the lowerer inlines literals) → JDBC
  # PreparedStatement. Because `with_connection` leases a Conn to exactly
  # one thread for a request's duration, the cache needs no lock.
  class Conn
    attr_reader :raw, :stmt_cache

    def initialize(raw)
      @raw = raw
      @stmt_cache = {}
    end
  end

  # Opaque per-prepare handle. Holds the (possibly cached) JDBC
  # PreparedStatement, the lazily-executed ResultSet, an `executed` latch
  # so repeated `step?`s don't re-run the query, and whether the
  # PreparedStatement is cached (kept open) or transient (closed on
  # finalize).
  class Stmt
    attr_accessor :pstmt, :rs, :executed, :cached

    def initialize(pstmt, cached)
      @pstmt = pstmt
      @rs = nil
      @executed = false
      @cached = cached
    end
  end

  @free      = nil
  @all       = nil
  @mutex     = nil
  @cv        = nil
  # Query-log capture (issue #27) — see db_cruby.rb. `nil` ⇒ not
  # capturing; an Array ⇒ accumulate each issued SQL string.
  @query_log = nil

  # Pool size defaults to the Puma thread count (RAILS_MAX_THREADS) so
  # every concurrently-serving thread can hold its own JDBC connection.
  def self.configure(path, pool_size: ENV.fetch("RAILS_MAX_THREADS", "3").to_i)
    @mutex = Mutex.new
    @cv    = ConditionVariable.new
    @free  = []
    @all   = []
    ds = SQLiteDataSource.new
    ds.set_url("jdbc:sqlite:#{path}")
    pool_size.times do
      raw = ds.get_connection
      raw.set_auto_commit(true)
      conn = Conn.new(raw)
      @free << conn
      @all  << conn
    end
  end

  # The Conn this thread should read/write through. Set by
  # `with_connection` (request scope); falls back to the pool's first
  # connection for single-thread test/dev modes. `Fiber[:k]` is
  # fiber-storage — under Puma's thread-per-request it is effectively
  # thread-local (each worker thread's root fiber).
  def self.current_dbh
    c = Fiber[:db_handle]
    return c unless c.nil?
    @free[0]
  end

  # Request-scoped connection lease. Mirrors db_cruby.rb: checks out a
  # Conn under @mutex (parking on @cv while the pool is momentarily
  # exhausted), binds it to fiber-storage so `current_dbh` resolves to it
  # for the block, and returns it on completion (even on raise).
  def self.with_connection
    conn = nil
    @mutex.synchronize do
      while @free.empty?
        @cv.wait(@mutex)
      end
      conn = @free.pop
    end
    Fiber[:db_handle] = conn
    begin
      yield
    ensure
      Fiber[:db_handle] = nil
      @mutex.synchronize do
        @free.push(conn)
        @cv.signal
      end
    end
  end

  def self.close
    return if @all.nil?
    @all.each do |conn|
      conn.stmt_cache.each_value { |ps| ps.close }
      conn.raw.close
    end
    @free = nil
    @all  = nil
  end

  def self.exec(sql)
    record_query(sql)
    st = current_dbh.raw.create_statement
    begin
      st.execute(sql)
    ensure
      st.close
    end
    nil
  end

  # Prepared-statement cache (roundhouse#12). A cache hit reuses the open
  # PreparedStatement (a fresh `executeQuery` in `step?` yields a new
  # ResultSet, so no explicit rewind is needed); `finalize` closes only
  # the ResultSet and keeps the cached statement. Over-cap statements are
  # transient and closed on finalize. Key is the composed SQL — inlined
  # literals key id-bearing queries per-id (fine for the bench;
  # STMT_CACHE_CAP bounds growth).
  def self.prepare(sql)
    record_query(sql)
    conn   = current_dbh
    cache  = conn.stmt_cache
    pstmt  = cache[sql]
    cached = true
    if pstmt.nil?
      pstmt = conn.raw.prepare_statement(sql)
      if cache.size < STMT_CACHE_CAP
        cache[sql] = pstmt
      else
        cached = false
      end
    end
    Stmt.new(pstmt, cached)
  end

  # Run the query exactly once, lazily. `sqlite_adapter.rb`'s `select_rows`
  # calls `column_count` before the first `step?`, so either entry point
  # may be first to need a live ResultSet — execute on whichever wins and
  # latch it so the other reuses the same cursor.
  def self.ensure_executed(stmt)
    return if stmt.executed
    stmt.rs = stmt.pstmt.execute_query
    stmt.executed = true
  end

  def self.step?(stmt)
    ensure_executed(stmt)
    stmt.rs.next
  end

  def self.column_int(stmt, i)
    stmt.rs.get_int(i + 1)
  end

  def self.column_text(stmt, i)
    v = stmt.rs.get_string(i + 1)
    v.nil? ? "" : v.to_s
  end

  # Raw typed column read (see db_cruby.rb): JDBC getObject gives the
  # driver's native value — Integer/Long for INTEGER affinity, Double
  # for REAL, String for TEXT, nil for NULL. Normalize java.lang
  # numerics via to_i/to_f pass-through is unnecessary — JRuby coerces
  # them to Ruby Integer/Float on comparison and arithmetic.
  def self.column_value(stmt, i)
    stmt.rs.get_object(i + 1)
  end

  # Read column metadata from the ResultSet (valid once the query has run
  # but before the first row is fetched). Universally supported across
  # JDBC drivers — unlike PreparedStatement.getMetaData(), whose
  # pre-execution behaviour varies. `ensure_executed` makes a
  # `column_count`-before-`step?` call order work.
  def self.column_count(stmt)
    ensure_executed(stmt)
    stmt.rs.get_meta_data.get_column_count
  end

  def self.column_name(stmt, i)
    ensure_executed(stmt)
    stmt.rs.get_meta_data.get_column_name(i + 1)
  end

  # Release the per-call handle. Close the ResultSet (if a query ran); a
  # cached PreparedStatement stays open for reuse, a transient one is
  # closed.
  def self.finalize(stmt)
    stmt.rs.close if stmt.rs
    stmt.pstmt.close unless stmt.cached
  end

  def self.last_insert_rowid
    st = current_dbh.raw.create_statement
    rs = st.execute_query("SELECT last_insert_rowid()")
    rs.next
    v = rs.get_long(1)
    rs.close
    st.close
    v
  end

  def self.changes
    st = current_dbh.raw.create_statement
    rs = st.execute_query("SELECT changes()")
    rs.next
    v = rs.get_int(1)
    rs.close
    st.close
    v
  end

  # Query-log capture — identical to db_cruby.rb (issue #27). Records the
  # SQL every prepare/exec issues during the block, returns it as an
  # Array; nestable. Production never calls this, so `record_query` stays
  # a single nil check off the hot path.
  def self.capture_sql
    prev = @query_log
    log  = []
    @query_log = log
    begin
      yield
    ensure
      @query_log = prev
    end
    log
  end

  def self.record_query(sql)
    @query_log.push(sql) unless @query_log.nil?
  end

  # SQL-value escaping primitives — copied verbatim from db_cruby.rb. The
  # contract across all shims is "inline values into SQL" (the FFI shim
  # can't construct SQLITE_TRANSIENT for bind params), and the lowerer
  # controls every string that flows here.
  def self.escape_string(s)
    "'" + s.to_s.gsub("'", "''") + "'"
  end

  def self.escape_int(n)
    n.to_i.to_s
  end

  def self.escape_int_list(ids)
    return "NULL" if ids.empty?

    ids.map { |i| i.to_i.to_s }.join(", ")
  end

  def self.escape_bool(b)
    b ? "1" : "0"
  end

  def self.column_bool(stmt, idx)
    column_int(stmt, idx) != 0
  end
end
