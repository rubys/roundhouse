# Primitive Db surface — JRuby variant. Same `module Db` contract as
  # The query cache keeps results up to this many rows — see db.rb.
  QC_CAPTURE_ROWS = 16

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
#   Db.bind_int(stmt, i, value) — bind an integer at a one-based position
#   Db.bind_text(stmt, i, value) — bind text at a one-based position
#   Db.bind_bool(stmt, i, value) — bind a boolean as SQLite's 0/1
#   Db.step?(stmt)             — advance, returns true if a row arrived
#   Db.column_int(stmt, i)     — read int column at zero-based index
#   Db.column_text(stmt, i)    — read text column at zero-based index
#   Db.column_count(stmt)      — number of columns in the prepared row
#   Db.column_name(stmt, i)    — name of column at zero-based index
#   Db.finalize(stmt)          — release the prepared stmt
#   Db.last_insert_rowid       — id of the last INSERTed row
#   Db.changes                 — affected-row count of the last statement
#
# THREAD-SAFETY: JRuby has no GVL, so Puma's worker threads run truly in
# parallel. db_cruby.rb once keyed statement handles into a single global
# `@rows` hash with a shared `@next_id` counter — safe only under CRuby's
# GVL, and it raced here and under TruffleRuby until it adopted this
# file's design. Here the stmt handle is an opaque `Stmt` wrapper object
# returned straight from `prepare`; callers only ever pass it back to
# `Db.*` (verified against `sqlite_adapter.rb`), so there is no shared
# mutable handle table to race on. The per-connection prepared-statement cache lives on the
# leased `Conn` (one thread at a time via `with_connection`), so it needs
# no lock either — same invariant db_cruby.rb relies on.
#
# JDBC notes: column indices are 1-based (we add 1 to the zero-based
# contract index). Execution is deferred until `step?` or a metadata
# read so all parameters can be bound after `prepare` returns.

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
  # keyed by SQL (including placeholders for bound values) → JDBC
  # PreparedStatement. Because `with_connection` leases a Conn to exactly
  # one thread for a request's duration, the cache needs no lock.
  class Conn
    attr_reader :raw, :stmt_cache, :open_statements

    def initialize(raw)
      @raw = raw
      @stmt_cache = {}
      @open_statements = {}.compare_by_identity
    end
  end

  # Opaque per-prepare handle. Holds the (possibly cached) JDBC
  # PreparedStatement, the lazily-executed ResultSet, an `executed` latch
  # so repeated `step?`s don't re-run the query, and whether the
  # PreparedStatement is cached (kept open) or transient (closed on
  # finalize).
  #
  # The query cache (see `query_cache_begin`) rides on the same handle:
  # `capture` collects a real statement's rows as they are stepped, for
  # the request's cache; `replay` is the cached result a later identical
  # SELECT answers from, `row`/`pos` its cursor, and `sql` the key both
  # need. A replay handle has no PreparedStatement until it is promoted
  # (see `step?`).
  class Stmt
    attr_accessor :pstmt, :rs, :executed, :cached, :sql, :capture, :replay, :row, :pos, :open

    def initialize(pstmt, cached)
      @pstmt = pstmt
      @rs = nil
      @executed = false
      @cached = cached
      @sql = nil
      @capture = nil
      @replay = nil
      @row = nil
      @pos = 0
      @open = nil
    end
  end

  @free      = nil
  @all       = nil
  @mutex     = nil
  @cv        = nil
  @quarantined = []
  @missing_connections = 0
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
    @quarantined = []
    @missing_connections = 0
    @path = path
    pool_size.times do
      conn = open_connection
      @free << conn
      @all  << conn
    end
  end

  def self.open_connection
    ds = SQLiteDataSource.new
    ds.set_url("jdbc:sqlite:#{@path}")
    raw = ds.get_connection
    raw.set_auto_commit(true)
    # STATED, not inherited — see the note in db_cruby.rb's open_pool.
    # The three Db shims must not agree on durability by whichever
    # SQLite each happened to link.
    st = raw.create_statement
    st.execute("PRAGMA journal_mode=WAL")
    st.execute("PRAGMA synchronous=NORMAL")
    st.execute("PRAGMA busy_timeout=5000")
    st.close
    Conn.new(raw)
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
  # Whether this thread is inside a `with_connection` lease. What
  # `Rails::Executor#wrap` asks before taking one: Rails' executor is
  # re-entrant, and a second lease on a thread that holds one would
  # rebind the connection and, on release, unbind the outer lease's.
  def self.in_lease?
    !Fiber[:db_handle].nil?
  end

  def self.with_connection
    conn = nil
    @mutex.synchronize do
      while @free.empty?
        if @missing_connections > 0
          replace_connection
        else
          @cv.wait(@mutex)
        end
      end
      conn = @free.pop
    end
    Fiber[:db_handle] = conn
    request_failed = false
    begin
      yield
    rescue Exception
      request_failed = true
      raise
    ensure
      cleanup_error = nil
      begin
        release_open_statements(conn)
      rescue StandardError => e
        cleanup_error = e
      ensure
        Fiber[:db_handle] = nil
        begin
          if conn.open_statements.empty?
            @mutex.synchronize do
              @free.push(conn)
              @cv.signal
            end
          else
            quarantine_connection(conn)
          end
        rescue StandardError => e
          cleanup_error ||= e
        end
      end
      raise cleanup_error if cleanup_error && !request_failed
    end
  end

  def self.quarantine_connection(conn)
    @mutex.synchronize do
      @quarantined << conn
      @missing_connections += 1
      begin
        # Open first so a shared in-memory database survives replacement.
        replace_connection
      ensure
        # A failed opener must also wake waiters so checkout can retry it
        # or raise, rather than wait for a connection that cannot check in.
        @cv.broadcast
      end
    end
    close_connection(conn)
  end

  # Called under @mutex; a failed open leaves the missing capacity intact.
  def self.replace_connection
    replacement = open_connection
    @all << replacement
    @free << replacement
    @missing_connections -= 1
  end

  def self.close_connection(conn)
    error = nil
    begin
      release_open_statements(conn)
    rescue StandardError => e
      error = e
    end
    conn.stmt_cache.each_value do |pstmt|
      begin
        pstmt.close unless pstmt.is_closed
      rescue StandardError => e
        error ||= e
      end
    end
    begin
      conn.raw.close unless conn.raw.is_closed
    rescue StandardError => e
      error ||= e
    end
    error
  end

  def self.close
    return if @all.nil?
    error = nil
    @all.each do |conn|
      close_error = close_connection(conn)
      error ||= close_error
    end
    @free = nil
    # A closed connection can still own idle statements whose close failed.
    remaining = @all.reject do |conn|
      conn.raw.is_closed && conn.open_statements.empty? &&
        conn.stmt_cache.each_value.all?(&:is_closed)
    end
    @quarantined = remaining
    @all = remaining.empty? ? nil : remaining
    raise error if error
  end

  def self.exec(sql)
    record_query(sql)
    # Any exec is (per the Db contract) DDL or a write — Rails
    # invalidates the whole query cache on write; so do we.
    qcache = Fiber[:rh_qcache]
    qcache.clear unless qcache.nil?
    st = current_dbh.raw.create_statement
    begin
      st.execute(sql)
    rescue StandardError => e
      raise ActiveRecord::RecordNotUnique, e.message if Db.unique_violation?(e.message)
      raise
    ensure
      st.close
    end
    nil
  end

  # A write that returns rows (`INSERT … RETURNING`, roundhouse#91),
  # as in db_cruby.rb: the query cache is cleared, every row is read
  # before the statement closes, and the handle replays them (step,
  # column_*, finalize as for a read). `changes` is the write's row count.
  def self.exec_returning(sql)
    v = loaded_sqlite_version
    if !returning_supported?(v)
      raise "Db.exec_returning: RETURNING needs SQLite 3.35 or newer; this driver bundles " + v.to_s
    end
    record_query(sql)
    qcache = Fiber[:rh_qcache]
    qcache.clear unless qcache.nil?
    rows = []
    names = []
    st = current_dbh.raw.create_statement
    begin
      if st.execute(sql)
        rs = st.get_result_set
        begin
          md = rs.get_meta_data
          n = md.get_column_count
          names = (1..n).map { |k| md.get_column_name(k) }
          while rs.next
            rows << (1..n).map { |k| rs.get_object(k) }
          end
        ensure
          rs.close
        end
      end
    rescue StandardError => e
      raise ActiveRecord::RecordNotUnique, e.message if Db.unique_violation?(e.message)
      raise
    ensure
      st.close
    end
    handle = Stmt.new(nil, false)
    handle.sql = sql
    handle.replay = { rows: rows, names: names, eof: true }
    handle
  end

  # Test-only hook, matching the Spinel SQLite shim's (db.rb) accessor of
  # the same name: always 0 here, since an exec_returning handle is a
  # plain Stmt object — there is no persistent array of outstanding
  # captures to leak.
  def self.qc_cursor_count
    0
  end

  # RETURNING arrived in SQLite 3.35.0 (3035000). An older library gets
  # a clear error rather than a syntax error, as #91 agreed.
  def self.returning_supported?(version_number)
    version_number >= 3035000
  end

  # The SQLite the JDBC driver bundles, as 3035000 for 3.35.0. Asked
  # once per process.
  def self.loaded_sqlite_version
    @sqlite_version ||= begin
      st = current_dbh.raw.create_statement
      begin
        rs = st.execute_query("SELECT sqlite_version()")
        rs.next
        major, minor, patch = rs.get_string(1).to_s.split(".").map(&:to_i)
        rs.close
        major * 1_000_000 + minor.to_i * 1000 + patch.to_i
      ensure
        st.close
      end
    end
  end

  # A UNIQUE-index violation is `ActiveRecord::RecordNotUnique`, not
  # whatever this driver raises. Rails' contract is what apps write
  # against — campfire's sign-up rescues it to turn a lost race into a
  # redirect to the login screen, and its first-run screen does the same
  # for two people opening a brand-new install at once. Without the
  # mapping the rescue never matched and the raw driver error reached
  # the dispatcher as a 500.
  #
  # THE TEST IS SQLITE'S OWN MESSAGE, not the driver's exception class,
  # and that is deliberate: "UNIQUE constraint failed: users.
  # email_address" comes out of the engine, so the same string appears
  # in the cruby gem's ConstraintException, in the JDBC SQLException and
  # in `sqlite3_errmsg` under spinel. One rule, three drivers, no
  # per-driver class table to keep in step. (The strict targets carry
  # their own `Db` and their own mapping; this is the ruby-family half.)
  def self.unique_violation?(message)
    message.to_s.include?("UNIQUE constraint failed")
  end

  # Prepared-statement cache (roundhouse#12). A cache hit reuses the open
  # PreparedStatement only when idle. Xerial's `executeQuery` in `step?`
  # resets the cursor before execution; release clears old parameters.
  # `finalize` closes the ResultSet and keeps the cached statement. Busy
  # hits and over-cap statements are transient, closed on finalize.
  # Placeholder queries reuse one cached statement across bound values;
  # STMT_CACHE_CAP bounds growth for other SQL shapes.
  def self.prepare(sql)
    # A `?`-bearing SQL string is a placeholder query (roundhouse#12):
    # its result depends on binds set after prepare, which are not in
    # the key, so it never joins the result-replay cache.
    parameterized = sql.include?("?")
    qcache = Fiber[:rh_qcache]
    if !qcache.nil? && !parameterized && (hit = qcache[sql])
      # A replay is not a round trip, and `capture_sql` does not count
      # it — Rails' SQLCounter skips CACHE events, and so do the other
      # two shims.
      st = Stmt.new(nil, false)
      st.sql = sql
      st.replay = hit
      return st
    end
    record_query(sql)
    conn   = current_dbh
    cache  = conn.stmt_cache
    pstmt  = cache[sql]
    cached = true
    if !pstmt.nil? && conn.open_statements.key?(pstmt)
      pstmt = conn.raw.prepare_statement(sql)
      cached = false
    elsif pstmt.nil?
      pstmt = conn.raw.prepare_statement(sql)
      if cache.size < STMT_CACHE_CAP
        cache[sql] = pstmt
      else
        cached = false
      end
    end
    st = Stmt.new(pstmt, cached)
    st.open = conn.open_statements
    st.open[pstmt] = st
    st.sql = sql
    st.capture = { rows: [], names: nil, eof: false } if !qcache.nil? && !parameterized
    st
  end

  # Explicit uncached reads skip statement reuse, retaining result replay.
  # finalize closes real transient statements, including replay promotion.
  def self.prepare_uncached(sql)
    qcache = Fiber[:rh_qcache]
    parameterized = sql.include?("?")
    if !qcache.nil? && !parameterized && (hit = qcache[sql])
      st = Stmt.new(nil, false)
      st.sql = sql
      st.replay = hit
      return st
    end
    record_query(sql)
    conn = current_dbh
    pstmt = conn.raw.prepare_statement(sql)
    st = Stmt.new(pstmt, false)
    st.open = conn.open_statements
    st.open[pstmt] = st
    st.sql = sql
    st.capture = { rows: [], names: nil, eof: false } if !qcache.nil? && !parameterized
    st
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
    if (hit = stmt.replay)
      if stmt.pos < hit[:rows].length
        stmt.row = hit[:rows][stmt.pos]
        stmt.pos += 1
        return true
      end
      return false if hit[:eof]
      # Cached prefix exhausted without eof (the first consumer stopped
      # early) — promote to a real transient statement, fast-forwarded
      # past the rows already replayed.
      conn = current_dbh
      pstmt = conn.raw.prepare_statement(stmt.sql)
      stmt.pstmt = pstmt
      stmt.cached = false
      stmt.replay = nil
      stmt.open = conn.open_statements
      stmt.open[pstmt] = stmt
      ensure_executed(stmt)
      stmt.pos.times { stmt.rs.next }
      return stmt.rs.next
    end
    ensure_executed(stmt)
    ok = stmt.rs.next
    if (c = stmt.capture)
      if ok && c[:rows].length >= QC_CAPTURE_ROWS
        # Bounded like the other two shims (db.rb `QC_CAPTURE_ROWS`).
        stmt.capture = nil
      elsif ok
        c[:names] = column_names_of(stmt) if c[:names].nil?
        n = c[:names].length
        row = Array.new(n)
        i = 0
        while i < n
          row[i] = stmt.rs.get_object(i + 1)
          i += 1
        end
        c[:rows] << row
      else
        c[:eof] = true
      end
    end
    ok
  rescue StandardError => e
    statement_failed(stmt, "step", e)
  end

  def self.column_names_of(stmt)
    md = stmt.rs.get_meta_data
    n = md.get_column_count
    (1..n).map { |k| md.get_column_name(k) }
  end

  def self.column_int(stmt, i)
    return stmt.row[i].to_i if stmt.replay
    stmt.rs.get_int(i + 1)
  end

  def self.column_float(stmt, i)
    return stmt.row[i].to_f if stmt.replay
    stmt.rs.get_double(i + 1)
  end

  # Per-request query cache — Rails' Active Record query cache: an
  # identical SELECT within one request replays the first result, and
  # any write (`exec`) empties the cache. The shared overlay dispatch
  # brackets every request with these two calls. Fiber storage is
  # thread-local under Puma's thread-per-request, so each worker thread
  # has its own cache. Same design as db_cruby.rb and db.rb.
  def self.query_cache_begin
    Fiber[:rh_qcache] = {}
  end

  def self.query_cache_end
    Fiber[:rh_qcache] = nil
  end

  # Is the replay cache on for this fiber? (`ActiveRecord::Base.uncached`)
  def self.query_cache_enabled?
    !Fiber[:rh_qcache].nil?
  end

  # The request read snapshot and background checkpoints are
  # implemented in the CRuby and Spinel shims (db_cruby.rb, db.rb), not
  # yet in this one. Here they
  # are accepted and do nothing, so the shared dispatcher and test
  # harness call them unconditionally; this lane still reads in
  # autocommit and checkpoints inside COMMIT.
  def self.read_snapshot_begin
    nil
  end

  def self.read_snapshot_end
    nil
  end

  def self.checkpoint_in_background!
    nil
  end

  def self.column_text(stmt, i)
    if stmt.replay
      v = stmt.row[i]
      return v.nil? ? "" : v.to_s
    end
    v = stmt.rs.get_string(i + 1)
    v.nil? ? "" : v.to_s
  end

  # Nullable-column reads (see db_cruby.rb): NULL stays nil instead of
  # collapsing to the type's zero. JDBC reports NULL out-of-band —
  # `getObject` is nil, and `wasNull` after a typed get — so read the
  # object first and only then coerce.
  def self.column_int_opt(stmt, i)
    v = stmt.replay ? stmt.row[i] : stmt.rs.get_object(i + 1)
    v.nil? ? nil : v.to_i
  end

  def self.column_float_opt(stmt, i)
    v = stmt.replay ? stmt.row[i] : stmt.rs.get_object(i + 1)
    v.nil? ? nil : v.to_f
  end

  def self.column_text_opt(stmt, i)
    v = stmt.replay ? stmt.row[i] : stmt.rs.get_string(i + 1)
    v.nil? ? nil : v.to_s
  end

  def self.column_bool_opt(stmt, i)
    v = stmt.replay ? stmt.row[i] : stmt.rs.get_object(i + 1)
    v.nil? ? nil : v.to_i != 0
  end

  # Raw typed column read (see db_cruby.rb): JDBC getObject gives the
  # driver's native value — Integer/Long for INTEGER affinity, Double
  # for REAL, String for TEXT, nil for NULL. Normalize java.lang
  # numerics via to_i/to_f pass-through is unnecessary — JRuby coerces
  # them to Ruby Integer/Float on comparison and arithmetic.
  def self.column_value(stmt, i)
    return stmt.row[i] if stmt.replay
    stmt.rs.get_object(i + 1)
  end

  # Read column metadata from the ResultSet (valid once the query has run
  # but before the first row is fetched). Universally supported across
  # JDBC drivers — unlike PreparedStatement.getMetaData(), whose
  # pre-execution behaviour varies. `ensure_executed` makes a
  # `column_count`-before-`step?` call order work.
  def self.column_count(stmt)
    return stmt.replay[:names].length if stmt.replay
    ensure_executed(stmt)
    stmt.rs.get_meta_data.get_column_count
  rescue StandardError => e
    statement_failed(stmt, "step", e)
  end

  def self.column_name(stmt, i)
    return stmt.replay[:names][i] if stmt.replay
    ensure_executed(stmt)
    stmt.rs.get_meta_data.get_column_name(i + 1)
  rescue StandardError => e
    statement_failed(stmt, "step", e)
  end

  # Release the per-call handle. A pure replay held no statement. A
  # capture is published to the request's query cache on release — even
  # a partial one (eof false): the next identical SELECT replays the
  # consumed prefix and promotes past it only if it wants more. Then
  # close the ResultSet (if a query ran); a cached PreparedStatement
  # stays open with no bound values for reuse, a transient one is closed.
  def self.finalize(stmt)
    return nil if stmt.pstmt.nil?
    if (c = stmt.capture)
      # A capture that never stepped has no column names yet; the next
      # consumer would find an empty, eof-less prefix and promote, which
      # is correct but pointless — leave it out.
      qcache = Fiber[:rh_qcache]
      if !c[:names].nil? && !qcache.nil? && !qcache.key?(stmt.sql)
        qcache[stmt.sql] = c
      end
    end
    release_statement(stmt)
  end

  # Lease cleanup also covers raises before finalize and replay promotion;
  # abandoned captures must not enter the request's result cache.
  def self.release_open_statements(conn)
    error = nil
    conn.open_statements.each_value do |stmt|
      begin
        release_statement(stmt, conn)
      rescue StandardError => e
        error ||= e
      end
    end
    raise error if error
  end

  def self.release_statement(stmt, conn = nil)
    pstmt = stmt.pstmt
    return nil if pstmt.nil?
    error = nil
    begin
      stmt.rs.close if stmt.rs
    rescue StandardError => e
      error = e
    end
    if stmt.cached
      begin
        pstmt.clear_parameters
      rescue StandardError => e
        error ||= e
      end
      if error
        conn ||= current_dbh
        cache = conn.stmt_cache
        cache.delete(stmt.sql) if cache[stmt.sql].equal?(pstmt)
        stmt.cached = false
      end
    end
    unless stmt.cached
      begin
        pstmt.close
      rescue StandardError => e
        error ||= e
      end
    end
    if error.nil? || (pstmt.is_closed && (stmt.rs.nil? || stmt.rs.is_closed))
      stmt.open.delete(pstmt)
      stmt.pstmt = nil
      stmt.rs = nil
    end
    stmt.capture = nil
    raise error if error
    nil
  end

  def self.statement_failed(stmt, _operation, error)
    stmt.replay = nil
    # Xerial can close the native statement after an execute error. Evict
    # the failed checkout instead of leaving a poisoned cache entry.
    cache = current_dbh.stmt_cache
    cache.delete(stmt.sql) if cache[stmt.sql].equal?(stmt.pstmt)
    stmt.cached = false
    begin
      release_statement(stmt)
    rescue StandardError
      # Failed closes remain owned until lease cleanup quarantines them.
    end
    raise error
  end

  # Optional primitives preserve nil for callers binding a SQL NULL.
  # Generated nullable equality uses IS NULL without a slot, or = ? with a slot.
  def self.bind_int_opt(handle, idx, value)
    ps = handle.pstmt
    raise "statement is not bindable" if ps.nil? || handle.executed
    if value.nil?
      ps.set_null(idx, Java::JavaSql::Types::INTEGER)
    else
      ps.set_long(idx, value)
    end
  rescue StandardError => error
    statement_failed(handle, "bind", error)
  end

  def self.bind_text_opt(handle, idx, value)
    ps = handle.pstmt
    raise "statement is not bindable" if ps.nil? || handle.executed
    if value.nil?
      ps.set_null(idx, Java::JavaSql::Types::VARCHAR)
    else
      bind_text(handle, idx, value)
    end
  rescue StandardError => error
    statement_failed(handle, "bind", error)
  end

  def self.bind_bool_opt(handle, idx, value)
    bind_bool(handle, idx, value)
  end

  # Bind positions are one-based. Keep the full SQLite integer width.
  def self.bind_int(stmt, idx, value)
    raise "statement is not bindable" if stmt.pstmt.nil? || stmt.executed
    if value.nil?
      stmt.pstmt.set_null(idx, Java::JavaSql::Types::INTEGER)
    else
      stmt.pstmt.set_long(idx, value)
    end
  rescue StandardError => error
    statement_failed(stmt, "bind", error)
  end

  # Match the inline writer, including ASCII-only BINARY strings as TEXT.
  def self.bind_text(stmt, idx, value)
    pstmt = stmt.pstmt
    raise "statement is not bindable" if pstmt.nil? || stmt.executed
    value = value.to_s
    if value.include?("\0") || (value.encoding == Encoding::BINARY && !value.ascii_only?)
      pstmt.set_bytes(idx, value.to_java_bytes)
    else
      pstmt.set_string(idx, value)
    end
  rescue StandardError => error
    statement_failed(stmt, "bind", error)
  end

  # SQLite boolean values are integers, with NULL distinct from false/0.
  def self.bind_bool(stmt, idx, value)
    pstmt = stmt.pstmt
    raise "statement is not bindable" if pstmt.nil? || stmt.executed
    if value.nil?
      pstmt.set_null(idx, Java::JavaSql::Types::INTEGER)
    else
      pstmt.set_int(idx, value ? 1 : 0)
    end
  rescue StandardError => error
    statement_failed(stmt, "bind", error)
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

  # SQL-value escaping primitives — copied verbatim from db_cruby.rb.
  # Writes and queries without bound parameters inline their values;
  # the lowerer controls every string that flows here.
  # BYTES go out as a hex BLOB literal, `X'…'`. A NUL cannot ride a
  # quoted literal at all (it ends the SQL text: "unrecognized token"),
  # and a binary value stored as TEXT sorts before every BLOB, so a
  # `t.binary` column filled half one way and half the other orders
  # wrong. Bytes = BINARY-encoded and not plain ASCII (lobsters'
  # `[a, b, c].pack("CCC")` confidence_order), or any string holding a
  # NUL. An ASCII-only BINARY string stays text, as it always was.
  def self.escape_string(s)
    str = s.to_s
    if str.include?("\0") || (str.encoding == Encoding::BINARY && !str.ascii_only?)
      return "X'" + str.unpack1("H*") + "'"
    end
    "'" + str.gsub("'", "''") + "'"
  end

  def self.escape_int(n)
    n.to_i.to_s
  end

  # Nullable-column writes (see db_cruby.rb): nil renders NULL.
  def self.escape_string_opt(s)
    s.nil? ? "NULL" : escape_string(s)
  end

  def self.escape_int_opt(n)
    n.nil? ? "NULL" : escape_int(n)
  end

  def self.escape_float_opt(f)
    f.nil? ? "NULL" : f.to_f.to_s
  end

  def self.escape_bool_opt(b)
    b.nil? ? "NULL" : escape_bool(b)
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
