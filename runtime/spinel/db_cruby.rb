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
  @db      = nil
  @rows    = {}
  @next_id = 0

  def self.configure(path)
    @db = SQLite3::Database.new(path)
    @db.results_as_hash = false
  end

  def self.close
    @db.close if !@db.nil?
    @db = nil
  end

  def self.exec(sql)
    @db.execute(sql)
  end

  def self.prepare(sql)
    @next_id += 1
    @rows[@next_id] = { stmt: @db.prepare(sql), row: nil }
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

  def self.finalize(stmt_id)
    entry = @rows.delete(stmt_id)
    entry[:stmt].close if entry
  end

  def self.last_insert_rowid
    @db.last_insert_row_id
  end

  def self.changes
    @db.changes
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
