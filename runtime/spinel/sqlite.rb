# Primitive SQLite surface — the layer that per-model adapter code sits
# on top of. CRuby shim using the `sqlite3` gem; the FFI shim (which
# binds the same module name to libsqlite3 directly via spinel's
# `ffi_lib`/`ffi_func`) is awaiting matz/spinel#405 (bare-call
# resolution) before it can replace this under the spinel target.
#
# API (the contract both shims must satisfy):
#
#   Sqlite.configure(path)         — open a database (":memory:" for tests)
#   Sqlite.close                   — close the database
#   Sqlite.exec(sql)               — run DDL / INSERT / UPDATE / DELETE
#   Sqlite.prepare(sql)            — prepare a SELECT, returns stmt handle
#   Sqlite.step?(stmt)             — advance, returns true if a row arrived
#   Sqlite.column_int(stmt, i)     — read int column at zero-based index
#   Sqlite.column_text(stmt, i)    — read text column at zero-based index
#   Sqlite.finalize(stmt)          — release the prepared stmt
#   Sqlite.last_insert_rowid       — id of the last INSERTed row
#   Sqlite.changes                 — affected-row count of the last statement
#
# Stmt handles are opaque integers — under FFI they're real `:ptr` values,
# under this CRuby shim they're per-call ids that index into a table that
# also caches the most recently stepped row (so column_int / column_text
# can pick fields by index, mirroring the FFI column accessors).
#
# This file replaces the role of InMemoryAdapter for spinel-target tests
# once the per-model Level-3 adapter primitives are emitted by the lowerer
# on top of this surface.

require "sqlite3"

module Sqlite
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
end
