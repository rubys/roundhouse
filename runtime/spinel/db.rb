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

module Db
  @pool = nil

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
    @pool = ActiveRecord::ConnectionAdapters::ConnectionPool.new(n) do
      rc = SQL.sqlite3_open(path, SQL.db_out)
      if rc != SQL::OK
        # Best-effort error surface — sqlite3_errmsg requires a valid
        # db handle, which we don't have on open failure. The numeric
        # rc + path are the only signals we can raise pre-handle.
        raise "Db.configure: sqlite3_open(" + path + ") failed (" + rc.to_s + ")"
      end
      SQL.read_ptr(SQL.db_out)
    end
  end

  # The handle this fiber should read/write through. Set by
  # `with_connection` (request scope) when wired; falls back to the
  # pool's first free handle for single-fiber test/dev modes.
  # `Fiber[:k]` is spinel's per-fiber storage indexer (#577/#578).
  def self.current_dbh
    h = Fiber[:db_handle]
    return h if !h.nil?
    @pool.free[0]
  end

  # Request-scoped connection lease for the fiber-per-connection server.
  # Checks out a handle, binds it to this fiber's storage so current_dbh
  # resolves to it for the request, then returns it. No mutex: spinel
  # fibers are cooperative (no preemption), so checkout/checkin on the
  # free list is atomic between yields. On exhaustion the fiber parks via
  # Tep::Scheduler (cooperative yield) until a checkin frees one; with
  # pool_size >= max concurrent fibers the wait loop never trips.
  #
  # NOTE: no begin/ensure (not used elsewhere in spinel-compiled code), so
  # a raise inside the block leaks the handle — acceptable on the happy
  # path; revisit if the dispatch path starts raising under load.
  def self.with_connection
    while @pool.available_count == 0
      Tep::Scheduler.pause(0.001)
    end
    h = @pool.checkout
    Fiber[:db_handle] = h
    result = yield
    Fiber[:db_handle] = nil
    @pool.checkin(h)
    result
  end

  def self.close
    return if @pool.nil?
    i = 0
    while i < @pool.free.length
      SQL.sqlite3_close(@pool.free[i])
      i += 1
    end
    @pool = nil
  end

  # DDL + INSERT/UPDATE/DELETE. `sqlite3_exec` doesn't return rows;
  # callers that want last_insert_rowid / changes consult those
  # accessors immediately after.
  def self.exec(sql)
    rc = SQL.sqlite3_exec(current_dbh, sql, nil, nil, nil)
    if rc != SQL::OK
      raise "Db.exec failed (" + rc.to_s + "): " + SQL.sqlite3_errmsg(current_dbh) + " — sql: " + sql
    end
  end

  # Returns the stmt pointer; caller advances with `step?`, reads
  # columns with `column_int` / `column_text`, releases with
  # `finalize`. The -1 length argument lets sqlite measure the SQL
  # itself (NUL-terminated).
  def self.prepare(sql)
    rc = SQL.sqlite3_prepare_v2(current_dbh, sql, -1, SQL.stmt_out, nil)
    if rc != SQL::OK
      raise "Db.prepare failed (" + rc.to_s + "): " + SQL.sqlite3_errmsg(current_dbh) + " — sql: " + sql
    end
    SQL.read_ptr(SQL.stmt_out)
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

  def self.finalize(stmt)
    SQL.sqlite3_finalize(stmt)
  end

  def self.last_insert_rowid
    SQL.sqlite3_last_insert_rowid(current_dbh)
  end

  def self.changes
    SQL.sqlite3_changes(current_dbh)
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
