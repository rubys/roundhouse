# Primitive Db surface — the layer that per-model adapter code sits on
# top of. The contract is database-agnostic; this file is the SQLite-via-
# the-cruby-`sqlite3`-gem implementation. Other backends (sqlite via
# spinel FFI, postgres via libpq, etc.) implement the same `module Db`
# in sibling files; a future dispatcher picks one at require time. See
# project_level_3_adapter_emit.md.
#
# API (the contract every Db shim must satisfy):
#
#   Db.configure(path)         — open a database (":memory:" for tests)
#   Db.close                   — close the database
#   Db.exec(sql)               — run DDL / INSERT / UPDATE / DELETE
#   Db.prepare(sql)            — prepare a SELECT, returns stmt handle
#   Db.step?(stmt)             — advance, returns true if a row arrived
#   Db.column_int(stmt, i)     — read int column at zero-based index
#   Db.column_text(stmt, i)    — read text column at zero-based index
#   Db.column_count(stmt)      — number of columns in the prepared row
#   Db.column_name(stmt, i)    — name of column at zero-based index
#   Db.finalize(stmt)          — release the prepared stmt
#   Db.last_insert_rowid       — id of the last INSERTed row
#   Db.changes                 — affected-row count of the last statement
#
# Stmt handles are opaque integers — under spinel FFI they're real `:ptr`
# values, under this CRuby shim they're per-call ids that index into a
# table that also caches the most recently stepped row (so column_int /
# column_text can pick fields by index, mirroring the FFI column
# accessors).
#
# Per-database SQL dialect differences (placeholder syntax, RETURNING vs
# last_insert_rowid, etc.) live inside each shim or in a separate dialect
# helper consulted by the lowerer at SQL-composition time.
#
# The Db primitive surface backs the lowerer-emitted Level-3 per-model
# `_adapter_*` methods. This file is the CRuby (gem-backed) variant;
# `db.rb` in the same directory is the FFI variant the Spinel-AOT
# target compiles against.

require "sqlite3"

module Db
  @pool    = nil
  @rows    = {}
  @next_id = 0
  @mutex   = nil
  @cv      = nil
  # Per-connection prepared-statement cache bound (roundhouse#12). Beyond
  # this many distinct SQL strings on one connection, further statements
  # are transient (closed on finalize) rather than cached — bounds growth
  # when inlined literals make id-bearing queries key per-id.
  STMT_CACHE_CAP = 128
  # Query-log capture (issue #27). `nil` ⇒ not capturing; an Array ⇒
  # accumulate the SQL each prepare/exec issues. The funnel hook
  # `record_query` is near-free (one nil check) when not capturing, so
  # this stays out of the way on the production path.
  @query_log = nil

  # Pool size defaults to the Puma thread count (RAILS_MAX_THREADS) so
  # every concurrently-serving thread can hold its own handle without
  # contending. Override explicitly for tests.
  def self.configure(path, pool_size: ENV.fetch("RAILS_MAX_THREADS", "3").to_i)
    @mutex = Mutex.new
    @cv    = ConditionVariable.new
    @pool = ActiveRecord::ConnectionAdapters::ConnectionPool.new(pool_size) do
      db = SQLite3::Database.new(path)
      db.results_as_hash = false
      db
    end
  end

  # The SQLite3::Database this thread should read/write through. Set by
  # `with_connection` (request scope) when wired; falls back to the
  # pool's first free handle for single-thread test/dev modes.
  # `Fiber[:k]` is fiber-storage — under Puma's thread-per-request it is
  # effectively thread-local (each worker thread's root fiber).
  def self.current_dbh
    h = Fiber[:db_handle]
    return h if !h.nil?
    @pool.free[0]
  end

  # Request-scoped connection lease. Checks out a handle, binds it to
  # this thread's fiber-storage so `current_dbh` resolves to it for the
  # block's duration, and returns it on completion (even on raise).
  #
  # Thread-safe for the CRuby/Puma target where N worker threads share
  # one pool: the pool's free list (not itself thread-safe) is mutated
  # only under @mutex, and a thread parks on @cv when the pool is
  # momentarily exhausted (size < live requests) rather than raising.
  # With pool_size == thread count, the wait loop never trips.
  def self.with_connection
    h = nil
    @mutex.synchronize do
      while @pool.available_count == 0
        @cv.wait(@mutex)
      end
      h = @pool.checkout
    end
    Fiber[:db_handle] = h
    begin
      yield
    ensure
      Fiber[:db_handle] = nil
      @mutex.synchronize do
        @pool.checkin(h)
        @cv.signal
      end
    end
  end

  def self.close
    return if @pool.nil?
    i = 0
    while i < @pool.free.length
      conn = @pool.free[i]
      # Finalize cached statements before closing the connection (older
      # sqlite3-gem builds refuse to close with unfinalized statements).
      cache = conn.instance_variable_get(:@rh_stmt_cache)
      cache.each_value { |st| st.close } if cache
      conn.close
      i += 1
    end
    @pool = nil
  end

  def self.exec(sql)
    record_query(sql)
    current_dbh.execute(sql)
  end

  # Prepared-statement cache (roundhouse#12). A SQLite3::Statement is bound
  # to the connection it was prepared on, so the cache lives ON the
  # connection object — and since `with_connection` leases a connection to
  # exactly one thread at a time, the per-connection cache needs no extra
  # lock. A cache hit rewinds the stmt (`reset!`) instead of re-parsing the
  # SQL; `finalize` resets rather than closes, so the stmt stays cached
  # (real `close` runs at pool shutdown). Key is the composed SQL: inlined
  # literals mean id-bearing queries key per-id (fine for the bench;
  # STMT_CACHE_CAP bounds growth, beyond which statements are transient and
  # closed on finalize). Placeholder binding — the planned follow-on —
  # makes the key the static query shape.
  def self.prepare(sql)
    record_query(sql)
    conn  = current_dbh
    cache = conn.instance_variable_get(:@rh_stmt_cache)
    if cache.nil?
      cache = {}
      conn.instance_variable_set(:@rh_stmt_cache, cache)
    end
    stmt   = cache[sql]
    cached = true
    if stmt.nil?
      stmt = conn.prepare(sql)
      if cache.size < STMT_CACHE_CAP
        cache[sql] = stmt
      else
        cached = false
      end
    else
      # Reused: rewind the cursor before re-stepping (robust even if a
      # prior request raised before its finalize).
      stmt.reset!
    end
    @next_id += 1
    @rows[@next_id] = { stmt: stmt, row: nil, cached: cached }
    @next_id
  end

  def self.step?(stmt_id)
    entry = @rows[stmt_id]
    row = entry[:stmt].step
    entry[:row] = row
    !row.nil?
  end

  def self.column_int(stmt_id, i)
    @rows[stmt_id][:row][i].to_i
  end

  def self.column_text(stmt_id, i)
    v = @rows[stmt_id][:row][i]
    v.nil? ? "" : v.to_s
  end

  def self.column_count(stmt_id)
    @rows[stmt_id][:stmt].columns.length
  end

  def self.column_name(stmt_id, i)
    @rows[stmt_id][:stmt].columns[i]
  end

  # Release the per-call handle. A cached stmt is reset! (rewound + read
  # lock dropped) and kept for reuse; a transient (over-cap) stmt is closed.
  def self.finalize(stmt_id)
    entry = @rows.delete(stmt_id)
    return unless entry
    if entry[:cached]
      entry[:stmt].reset!
    else
      entry[:stmt].close
    end
  end

  def self.last_insert_rowid
    current_dbh.last_insert_row_id
  end

  def self.changes
    current_dbh.changes
  end

  # Query-log capture — the test-side analog of Rails'
  # `ActiveSupport::Notifications.subscribed(counter, "sql.active_record")`
  # (activerecord testing/query_assertions.rb). Records the SQL every
  # prepare/exec issues during the block and returns it as an Array of
  # SQL strings, the shape Rails' `capture_sql` yields. Nestable: an
  # outer capture is restored on exit. Production never calls this, so
  # the funnel hook stays a single nil check off the hot path.
  #
  # The only instrument that can see the `includes(:assoc)` N+1:
  # byte-identical `compare` is blind to it (eager-load and N+1 render
  # the same HTML; only the query strategy differs). See issue #27.
  def self.capture_sql
    prev = @query_log
    log = []
    @query_log = log
    begin
      yield
    ensure
      @query_log = prev
    end
    log
  end

  # Funnel hook: record one SQL string into the active capture, if any.
  # No-op (single nil check) when no capture is installed.
  def self.record_query(sql)
    @query_log.push(sql) unless @query_log.nil?
  end

  # SQL-value escaping primitives — lowerer-emitted code uses these
  # to compose SQL with inlined values. Spinel-FFI can't construct
  # SQLITE_TRANSIENT for bind_text, so the contract across both
  # runtimes is "inline values into SQL"; the cruby gem accepts that
  # form fine (no semantic difference vs bound params for safety
  # since the lowerer controls every string that flows here).
  def self.escape_string(s)
    "'" + s.to_s.gsub("'", "''") + "'"
  end

  def self.escape_int(n)
    n.to_i.to_s
  end

  # Render an integer list for `IN (...)` eager-load batches (issue
  # #27). Empty list → "NULL" so `IN (NULL)` is valid SQL matching no
  # rows (an empty `IN ()` is a syntax error).
  def self.escape_int_list(ids)
    return "NULL" if ids.empty?

    ids.map { |i| i.to_i.to_s }.join(", ")
  end

  # SQLite stores booleans as 0/1 integers (no native bool type) —
  # AR `t.boolean :col` maps to INTEGER affinity. Emit the inline
  # literal directly; saves a CAST round-trip vs `'true'`/`'false'`.
  def self.escape_bool(b)
    b ? "1" : "0"
  end

  # Read a boolean column. SQLite returns 0/1 (integer), we widen to
  # Ruby's bool. Nulls coerce to false.
  def self.column_bool(stmt_id, idx)
    column_int(stmt_id, idx) != 0
  end
end
