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
#   Db.bind_int/bind_text/bind_bool(stmt, i, v) — bind `?` param i (1-based)
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
  # Per-connection prepared-statement cache bound (roundhouse#12). The
  # cache is LRU (hits re-insert; at cap the oldest entry is closed and
  # evicted), so a working set larger than the cap degrades gracefully
  # instead of pinning the first N statements forever and re-parsing
  # everything else — profiling the lobsters bench showed the old
  # first-come-stays policy spending 13% of wall time in
  # sqlite3_prepare/close because inlined id literals key per-id. The
  # cap covers the lobsters sequence's per-iteration distinct-SQL
  # working set with room; a prepared stmt is ~KBs, so worst case is a
  # few MB per connection.
  STMT_CACHE_CAP = 4096
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
    # Any exec is (per the Db contract) DDL or a write — Rails
    # invalidates the whole query cache on write; so do we.
    qcache = Fiber[:rh_qcache]
    qcache.clear unless qcache.nil?
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
  # closed on finalize). With placeholder binding on (roundhouse#12) the
  # key is instead the static shape (`WHERE id = ?`), so id-varying
  # queries share one cached statement.
  # ── per-request SQL query cache (Rails AR query-cache semantics) ──
  # Identical SELECTs within one request replay the first result set;
  # any `exec` (writes ride exec per the Db contract) invalidates.
  # Fiber-local so Puma's thread-per-request can't cross-pollute.
  # Entries capture rows AS CONSUMED plus an eof flag: a point-lookup
  # reader that steps once caches one row + eof=false, and a later
  # consumer wanting more rows promotes to a real re-executed
  # statement (rare). Enabled per-request by the dispatch; nil ⇒ off
  # (tests, scripts) with a single fiber-storage read of overhead.
  def self.query_cache_begin
    Fiber[:rh_qcache] = {}
  end

  def self.query_cache_end
    Fiber[:rh_qcache] = nil
  end

  def self.prepare(sql)
    record_query(sql)
    # A `?`-bearing SQL string is a placeholder query (roundhouse#12):
    # its result depends on the runtime binds set AFTER prepare, which
    # aren't in the SQL key — so it must NOT participate in the
    # result-replay query cache (replaying would serve one bind value's
    # rows for another). The prepared-statement cache below still keys on
    # the shared shape, which is the whole point. (Heuristic: a literal
    # value containing `?` would also skip qcache — a safe miss, never a
    # wrong result.)
    parameterized = sql.include?("?")
    qcache = Fiber[:rh_qcache]
    if !qcache.nil? && !parameterized && (hit = qcache[sql])
      @next_id += 1
      @rows[@next_id] = { stmt: nil, row: nil, cached: false, replay: hit, pos: 0, sql: sql }
      return @next_id
    end
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
      if cache.size >= STMT_CACHE_CAP
        # Evict least-recently-used (Ruby Hash is insertion-ordered and
        # hits below re-insert, so the earliest key is the LRU). Skip
        # any statement still held by an open @rows handle — closing it
        # under a live cursor would break nested prepare patterns. If
        # everything is somehow in use, insert past the cap (soft
        # bound) rather than close a live statement.
        live = nil
        cache.each_key do |k|
          candidate = cache[k]
          next if @rows.any? { |_, e| e[:stmt].equal?(candidate) }
          live = k
          break
        end
        unless live.nil?
          cache.delete(live).close
        end
      end
      cache[sql] = stmt
    else
      # Reused: rewind the cursor before re-stepping (robust even if a
      # prior request raised before its finalize). Re-insert to record
      # recency (LRU discipline).
      cache.delete(sql)
      cache[sql] = stmt
      stmt.reset!
    end
    @next_id += 1
    capture = nil
    qcache = Fiber[:rh_qcache]
    unless qcache.nil? || parameterized
      capture = { rows: [], names: stmt.columns, eof: false, sql: sql }
    end
    @rows[@next_id] = { stmt: stmt, row: nil, cached: cached, capture: capture }
    @next_id
  end

  def self.step?(stmt_id)
    entry = @rows[stmt_id]
    if (hit = entry[:replay])
      if entry[:pos] < hit[:rows].length
        entry[:row] = hit[:rows][entry[:pos]]
        entry[:pos] += 1
        return true
      end
      return false if hit[:eof]
      # Cached prefix exhausted without eof (original consumer stopped
      # early) — promote to a real transient statement, fast-forwarded
      # past the rows already replayed.
      stmt = current_dbh.prepare(entry[:sql])
      entry[:pos].times { stmt.step }
      entry[:stmt] = stmt
      entry[:replay] = nil
      entry[:promoted] = true
      row = stmt.step
      entry[:row] = row
      return !row.nil?
    end
    row = entry[:stmt].step
    entry[:row] = row
    if (c = entry[:capture])
      if row.nil?
        c[:eof] = true
      else
        c[:rows] << row
      end
    end
    !row.nil?
  end

  def self.column_int(stmt_id, i)
    @rows[stmt_id][:row][i].to_i
  end

  def self.column_float(stmt_id, i)
    @rows[stmt_id][:row][i].to_f
  end

  def self.column_text(stmt_id, i)
    v = @rows[stmt_id][:row][i]
    v.nil? ? "" : v.to_s
  end

  # Raw typed column read: the value exactly as the sqlite3 gem
  # returns it — Integer for INTEGER affinity, Float for REAL, String
  # for TEXT, and crucially nil for NULL (column_text collapses NULL
  # to "", which breaks Rails semantics like `group_by(&:fk)[nil]`
  # and integer-column truthiness). Whole-row hydration
  # (SqliteAdapter.select_rows) reads through this so model attributes
  # carry real types, matching what ActiveRecord hands the app.
  def self.column_value(stmt_id, i)
    @rows[stmt_id][:row][i]
  end

  def self.column_count(stmt_id)
    e = @rows[stmt_id]
    return e[:replay][:names].length if e[:replay]
    e[:stmt].columns.length
  end

  def self.column_name(stmt_id, i)
    e = @rows[stmt_id]
    return e[:replay][:names][i] if e[:replay]
    e[:stmt].columns[i]
  end

  # Release the per-call handle. A cached stmt is reset! (rewound + read
  # lock dropped) and kept for reuse; a transient (over-cap or
  # replay-promoted) stmt is closed; a pure replay handle held no
  # statement at all. A capture is published to the request's query
  # cache on release — even a partial one (eof=false): the next
  # identical SELECT replays the consumed prefix and promotes past it
  # only if it wants more.
  def self.finalize(stmt_id)
    entry = @rows.delete(stmt_id)
    return unless entry
    return if entry[:replay] # replay handle — nothing to release
    if (c = entry[:capture])
      qcache = Fiber[:rh_qcache]
      qcache[c[:sql]] = c if !qcache.nil? && !qcache.key?(c[:sql])
    end
    if entry[:cached]
      entry[:stmt].reset!
    else
      entry[:stmt].close
    end
  end

  # Placeholder binding (roundhouse#12). Bind one `?` param (1-based) on
  # a prepared stmt before the first `step?`, via the gem's
  # `Statement#bind_param`. The emitted `_adapter_*` bodies always
  # re-bind every param before stepping, so a cached stmt's prior binds
  # are overwritten (SQLite `reset` keeps bindings; our re-bind replaces
  # them positionally — same param count for the same SQL shape). The
  # nil guard covers the replay-handle case, which `?` queries never take
  # (prepare skips replay for parameterized SQL).
  def self.bind_int(stmt_id, idx, value)
    st = @rows[stmt_id][:stmt]
    st.bind_param(idx, value) unless st.nil?
  end

  def self.bind_text(stmt_id, idx, value)
    st = @rows[stmt_id][:stmt]
    st.bind_param(idx, value) unless st.nil?
  end

  # SQLite has no native bool — bind 0/1, matching escape_bool's inline
  # form and the INTEGER affinity `t.boolean` columns get.
  def self.bind_bool(stmt_id, idx, value)
    st = @rows[stmt_id][:stmt]
    st.bind_param(idx, value ? 1 : 0) unless st.nil?
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

  # SQL-value escaping primitives — lowerer-emitted code uses these to
  # inline literals (and runtime values when the placeholder-bind gate is
  # off). With the gate on, runtime values instead flow through `bind_*`
  # above; both shims (this gem-backed one and spinel-FFI) now support
  # binding. Inlining stays safe regardless since the lowerer controls
  # every string that flows here.
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
