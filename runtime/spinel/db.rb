# Primitive Db surface — the layer that per-model adapter code sits on
# top of. The contract is database-agnostic; this file is the
# SQLite-via-libsqlite3-FFI implementation, the runtime spinel-compiled
# binaries use. The sqlite3-gem-backed sibling (`db_cruby.rb`) is the
# stock-CRuby implementation; both define `module Db` with the same
# external API. main.rb requires this file (the FFI variant);
# test_helper.rb requires `db_cruby` (the gem variant) so the existing
# `ruby -Itest test/...` developer loop keeps working under stock CRuby.
#
# API (the contract every Db shim must satisfy — must match db_cruby.rb):
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
#   Db.escape_string(s)        — SQL-quote a string value
#   Db.escape_int(n)           — render an integer for SQL inlining
#
# Stmt handles are opaque pointers (`:ptr`) returned by sqlite3_prepare_v2
# via the SQL.stmt_out out-buffer. The contract today is "inline values
# into SQL"; lowerer-emitted code goes through `escape_string` /
# `escape_int` (both shimmed below) before composition. `bind_text` is
# unblocked at the FFI layer (spinel #576 + matz/spinel#686 doc fix) —
# placeholder-bind emit is a planned follow-on.
#
# Module-level state is a single `@pool` (ActiveRecord ConnectionPool of N
# opaque dbh ptrs). `Db.current_dbh` reads Fiber.storage[:db_handle] when
# set (request-scoped checkout) and falls back to the pool's first free
# handle otherwise (single-fiber test/dev mode). Existing call sites don't
# care which path they're on; they just call Db.X.
#
# Pattern mirrors `examples/ffi/sqlite/blog.rb` in the spinel repo —
# the same `ffi_func` declarations, the same out-buffer plumbing.

# Bare-metal SQLite3 FFI bindings. Only the surface area Roundhouse's
# lowerer-emitted `_adapter_*` methods need.
module SQL
  ffi_lib "sqlite3"

  ffi_const :OK,   0
  ffi_const :ROW,  100
  ffi_const :DONE, 101

  ffi_func :sqlite3_open,              [:str, :ptr],                          :int
  ffi_func :sqlite3_close,             [:ptr],                                :int
  ffi_func :sqlite3_exec,              [:ptr, :str, :ptr, :ptr, :ptr],        :int
  ffi_func :sqlite3_prepare_v2,        [:ptr, :str, :int, :ptr, :ptr],        :int
  ffi_func :sqlite3_step,              [:ptr],                                :int
  ffi_func :sqlite3_finalize,          [:ptr],                                :int
  # Prepared-statement reuse (roundhouse#12, Path A.1). `reset` rewinds a
  # stepped stmt so it can be re-stepped; `clear_bindings` drops any bound
  # params. Cached `Db.finalize` calls these instead of `sqlite3_finalize`,
  # which now runs only at pool shutdown (see DbConn#finalize_all).
  ffi_func :sqlite3_reset,             [:ptr],                                :int
  ffi_func :sqlite3_clear_bindings,    [:ptr],                                :int
  ffi_func :sqlite3_column_int,        [:ptr, :int],                          :int
  ffi_func :sqlite3_column_text,       [:ptr, :int],                          :str
  ffi_func :sqlite3_column_count,      [:ptr],                                :int
  ffi_func :sqlite3_column_name,       [:ptr, :int],                          :str
  ffi_func :sqlite3_errmsg,            [:ptr],                                :str
  ffi_func :sqlite3_last_insert_rowid, [:ptr],                                :long
  ffi_func :sqlite3_changes,           [:ptr],                                :int

  # Out-params — sqlite3_open writes the db handle here, prepare_v2
  # writes the stmt handle. 8 bytes is enough for a 64-bit pointer.
  ffi_buffer :db_out,   8
  ffi_buffer :stmt_out, 8
  ffi_read_ptr :read_ptr, 0
end

# One pooled SQLite connection plus its prepared-statement cache
# (roundhouse#12). Prepared stmts are bound to the connection they were
# prepared on and carry their own cursor, so the cache MUST be
# per-connection: a global SQL->stmt map would let two cooperative fibers
# (each leasing a different pool handle) hand back the same stmt and
# corrupt each other's cursor mid-iteration. Because the pool leases a
# DbConn to exactly one fiber at a time, the per-connection cache is also
# concurrency-safe without a mutex.
#
# Cache key is the fully-composed SQL string. Today the lowerer inlines
# literals (`WHERE id = 1`), so id-bearing queries key per-id — fine for
# the fixed-id benchmark, and the CAP below bounds growth until
# placeholder-binding (the planned follow-on) makes the key the static
# query shape.
# One cache entry: the composed SQL and its prepared stmt ptr. A concrete
# user class (not a raw ptr) so an Array of these types concretely the way
# ConnectionPool's @free does — spinel infers the element type from the
# first `push`, which lets `length`/`[]` resolve (a poly_array of bare ptrs
# does not support them).
class Stmt
  def initialize(sql, ptr)
    @sql = sql
    @ptr = ptr
  end

  def sql
    @sql
  end

  def ptr
    @ptr
  end
end

class DbConn
  CAP = 128

  def initialize(dbh)
    @dbh = dbh
    @entries = []
  end

  def dbh
    @dbh
  end

  # Return a cached prepared stmt for `sql`, preparing+caching on miss.
  # Linear scan — the query set is ~8 shapes, so a scan beats a ptr-keyed
  # hash and avoids spinel hash-of-ptr typing.
  def prepare_cached(sql)
    i = 0
    while i < @entries.length
      e = @entries[i]
      return e.ptr if e.sql == sql
      i += 1
    end
    rc = SQL.sqlite3_prepare_v2(@dbh, sql, -1, SQL.stmt_out, nil)
    if rc != SQL::OK
      raise "Db.prepare failed (" + rc.to_s + "): " + SQL.sqlite3_errmsg(@dbh) + " — sql: " + sql
    end
    st = SQL.read_ptr(SQL.stmt_out)
    @entries.push(Stmt.new(sql, st)) if @entries.length < CAP
    st
  end

  # Real finalize of every cached stmt — pool-shutdown path only.
  def finalize_all
    i = 0
    while i < @entries.length
      SQL.sqlite3_finalize(@entries[i].ptr)
      i += 1
    end
  end
end

# Dedicated SQLite connection pool (roundhouse#12). A single-use object so
# its instance ivars type concretely: @conns is a DbConn PtrArray (objects
# keep their tag, unlike the generic ConnectionPool's int slot), @free is
# an IntArray stack of available indices into @conns.
class DbPool
  def initialize(path, n)
    @conns = []
    @free  = []
    i = 0
    while i < n
      rc = SQL.sqlite3_open(path, SQL.db_out)
      if rc != SQL::OK
        # Best-effort error surface — sqlite3_errmsg requires a valid db
        # handle, which we don't have on open failure. The numeric rc +
        # path are the only signals we can raise pre-handle.
        raise "Db.configure: sqlite3_open(" + path + ") failed (" + rc.to_s + ")"
      end
      @conns.push(DbConn.new(SQL.read_ptr(SQL.db_out)))
      @free.push(i)
      i += 1
    end
  end

  def available
    @free.length
  end

  # Pop a free connection index (LIFO).
  def lease
    @free.delete_at(@free.length - 1)
  end

  def release(idx)
    @free.push(idx)
  end

  def conn(idx)
    @conns[idx]
  end

  def first
    @conns[0]
  end

  # Finalize every cached stmt on every connection, then close the handles.
  def close_all
    i = 0
    while i < @conns.length
      c = @conns[i]
      c.finalize_all
      SQL.sqlite3_close(c.dbh)
      i += 1
    end
  end
end

# Temporal intrinsics (`ActiveSupport.parse_db_time` in the synthesized
# column readers, `db_now` in fill_timestamps) — chained off Db, the one
# require every persistence-touching bootstrap (main.rb AND the emitted
# test_helper) already loads. Mirrors the db_cruby/db_jruby chain the
# CRuby/JRuby materialization inserts; before this file existed the
# calls were unresolved and spinel's old silent gate nil'd them
# (spinel#1661 — the strict gate in spinel 1356cb14 surfaced it).
require_relative "active_support_time_parsing"

module Db
  # Own connection pool (roundhouse#12). Was
  # ActiveRecord::ConnectionAdapters::ConnectionPool, but that generic
  # stores handles in an sp_IntArray slot — a DbConn* flattens to a bare
  # machine word there and reads back tagged INT, so a later `.dbh` call
  # (guarded `tag == OBJ`) silently no-ops to NULL. A dedicated single-use
  # pool object (DbPool, below) keeps its connections in an INSTANCE-ivar
  # array, which spinel types as a concrete DbConn PtrArray (same shape as
  # DbConn#@entries) — preserving the object tag.
  @pool = nil
  # Query-log capture (issue #27). `nil` ⇒ not capturing; an Array ⇒
  # accumulate the SQL each prepare/exec issues. Kept in parity with the
  # cruby shim (db_cruby.rb); see `capture_sql` below.
  @query_log = nil

  # Pool size: kwarg wins; otherwise DATABASE_POOL_SIZE env (the same
  # knob the rust target reads — set it to the server's max concurrent
  # connections so no fiber parks waiting for a handle); else a modest
  # default. Each entry is one FFI sqlite3 handle to `path`.
  def self.configure(path, pool_size: 8)
    n = pool_size
    ev = ENV["DATABASE_POOL_SIZE"]
    if !ev.nil? && ev != ""
      n = ev.to_i
    end
    @pool = DbPool.new(path, n)
  end

  # The DbConn this fiber should read/write through. Set by
  # `with_connection` (request scope); falls back to the first connection
  # for single-fiber test/dev/boot (e.g. DDL) modes. `Fiber[:k]` is
  # spinel's per-fiber storage indexer (#577/#578). The stored value is a
  # real DbConn object (tag OBJ), so the `.dbh`/`.prepare_cached` calls on
  # the result resolve — unlike the int-boxed ConnectionPool path.
  def self.current_conn
    c = Fiber[:db_conn]
    return c if !c.nil?
    @pool.first
  end

  # Request-scoped connection lease for the fiber-per-connection server.
  # Leases a connection index, binds its DbConn to this fiber's storage,
  # runs the block, then releases the index. No mutex: spinel fibers are
  # cooperative (no preemption), so the lease/release are atomic between
  # yields. On exhaustion the fiber parks via Tep::Scheduler until a
  # release frees one; with pool_size >= max concurrent fibers the wait
  # loop never trips.
  #
  # NOTE: no begin/ensure (not used elsewhere in spinel-compiled code), so
  # a raise inside the block leaks the lease — acceptable on the happy
  # path; revisit if the dispatch path starts raising under load.
  def self.with_connection
    while @pool.available == 0
      Tep::Scheduler.pause(0.001)
    end
    idx = @pool.lease
    Fiber[:db_conn] = @pool.conn(idx)
    result = yield
    Fiber[:db_conn] = nil
    @pool.release(idx)
    result
  end

  def self.close
    return if @pool.nil?
    @pool.close_all
    @pool = nil
  end

  # DDL + INSERT/UPDATE/DELETE. `sqlite3_exec` doesn't return rows;
  # callers that want last_insert_rowid / changes consult those
  # accessors immediately after.
  def self.exec(sql)
    record_query(sql)
    h = current_conn.dbh
    rc = SQL.sqlite3_exec(h, sql, nil, nil, nil)
    if rc != SQL::OK
      raise "Db.exec failed (" + rc.to_s + "): " + SQL.sqlite3_errmsg(h) + " — sql: " + sql
    end
  end

  # Returns the stmt pointer; caller advances with `step?`, reads
  # columns with `column_int` / `column_text`, releases with
  # `finalize`. The -1 length argument lets sqlite measure the SQL
  # itself (NUL-terminated).
  def self.prepare(sql)
    record_query(sql)
    current_conn.prepare_cached(sql)
  end

  # Query-log capture — see db_cruby.rb for the full rationale (the
  # test-side analog of Rails' `sql.active_record` SQLCounter; the one
  # instrument that can see the includes(:assoc) N+1 `compare` is blind
  # to, issue #27). Kept in parity across both Db shims.
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

  def self.step?(stmt)
    SQL.sqlite3_step(stmt) == SQL::ROW
  end

  def self.column_int(stmt, i)
    SQL.sqlite3_column_int(stmt, i)
  end

  # The libsqlite3 column buffer is invalidated by the next step or
  # finalize on the same stmt. Force a copy by appending an empty
  # string so the value survives downstream use. Mirrors the pattern
  # in spinel's reference blog.rb FFI example.
  def self.column_text(stmt, i)
    s = SQL.sqlite3_column_text(stmt, i)
    if s.nil?
      ""
    else
      s + ""
    end
  end

  def self.column_count(stmt)
    SQL.sqlite3_column_count(stmt)
  end

  # libsqlite3's `sqlite3_column_name` returns a pointer owned by the
  # stmt; force a copy by appending an empty string so the value
  # survives the next step / finalize, mirroring `column_text`.
  def self.column_name(stmt, i)
    s = SQL.sqlite3_column_name(stmt, i)
    if s.nil?
      ""
    else
      s + ""
    end
  end

  # roundhouse#12 Path A.1: with caching on, "finalize" means rewind the
  # cached stmt (reset cursor + clear any bound params) so the next call
  # reuses it. Real sqlite3_finalize runs only at pool close.
  def self.finalize(stmt)
    SQL.sqlite3_reset(stmt)
    SQL.sqlite3_clear_bindings(stmt)
  end

  def self.last_insert_rowid
    SQL.sqlite3_last_insert_rowid(current_conn.dbh)
  end

  def self.changes
    SQL.sqlite3_changes(current_conn.dbh)
  end

  # Same SQL-value escaping shape as the gem-backed sibling. Single-
  # quote doubling matches sqlite's literal-string syntax; non-string
  # input goes through `to_s` first (Ruby semantics).
  def self.escape_string(s)
    "'" + s.to_s.gsub("'", "''") + "'"
  end

  def self.escape_int(n)
    n.to_i.to_s
  end

  # Render an integer list for `IN (...)` eager-load batches (issue
  # #27). Empty list → "NULL" so `IN (NULL)` is valid SQL matching no
  # rows (an empty `IN ()` is a syntax error). Mirrors the cruby shim.
  def self.escape_int_list(ids)
    return "NULL" if ids.empty?

    ids.map { |i| i.to_i.to_s }.join(", ")
  end

  # SQLite stores booleans as 0/1 integers (no native bool type) —
  # mirrors the cruby sibling shim.
  def self.escape_bool(b)
    b ? "1" : "0"
  end

  # Read a boolean column. SQLite returns 0/1 (integer), widen to bool.
  def self.column_bool(stmt, idx)
    column_int(stmt, idx) != 0
  end
end
