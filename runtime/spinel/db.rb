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
# via the SQL.stmt_out out-buffer. The FFI shim can't construct
# SQLITE_TRANSIENT for `bind_text`, so the contract is "inline values
# into SQL"; lowerer-emitted code goes through `escape_string` /
# `escape_int` (both shimmed below) before composition.
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
  @db = nil

  def self.configure(path)
    rc = SQL.sqlite3_open(path, SQL.db_out)
    if rc != SQL::OK
      # Best-effort error surface — sqlite3_errmsg requires a valid
      # db handle, which we don't have on open failure. The numeric
      # rc + path are the only signals we can raise pre-handle.
      raise "Db.configure: sqlite3_open(" + path + ") failed (" + rc.to_s + ")"
    end
    @db = SQL.read_ptr(SQL.db_out)
  end

  def self.close
    if !@db.nil?
      SQL.sqlite3_close(@db)
      @db = nil
    end
  end

  # DDL + INSERT/UPDATE/DELETE. `sqlite3_exec` doesn't return rows;
  # callers that want last_insert_rowid / changes consult those
  # accessors immediately after.
  def self.exec(sql)
    rc = SQL.sqlite3_exec(@db, sql, nil, nil, nil)
    if rc != SQL::OK
      raise "Db.exec failed (" + rc.to_s + "): " + SQL.sqlite3_errmsg(@db) + " — sql: " + sql
    end
  end

  # Returns the stmt pointer; caller advances with `step?`, reads
  # columns with `column_int` / `column_text`, releases with
  # `finalize`. The -1 length argument lets sqlite measure the SQL
  # itself (NUL-terminated).
  def self.prepare(sql)
    rc = SQL.sqlite3_prepare_v2(@db, sql, -1, SQL.stmt_out, nil)
    if rc != SQL::OK
      raise "Db.prepare failed (" + rc.to_s + "): " + SQL.sqlite3_errmsg(@db) + " — sql: " + sql
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
    SQL.sqlite3_last_insert_rowid(@db)
  end

  def self.changes
    SQL.sqlite3_changes(@db)
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
end
