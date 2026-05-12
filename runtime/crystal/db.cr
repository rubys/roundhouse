# Roundhouse Crystal DB runtime — sqlite primitive layer plus the
# `ActiveRecord.adapter` plug-in.
#
# Three responsibilities:
#   1. `Roundhouse::Db` — owns the sqlite3 connection. `open_production_db`
#      is called from `Roundhouse::Server.start`; `setup_test_db` resets
#      the connection between specs.
#   2. `Roundhouse::ActiveRecordAdapter` — abstract base pinning the 9-
#      method contract `runtime/ruby/active_record/base.rb` calls
#      (`all`, `find`, `where`, `count`, `exists?`, `insert`, `update`,
#      `delete`, `truncate`). Polymorphic slot so production sqlite,
#      test in-memory, and future libsql/D1 implementations all plug
#      into the same `ActiveRecord.adapter` setter.
#   3. `Roundhouse::SqliteAdapter` — concrete sqlite implementation.
#      Server boot assigns an instance to `ActiveRecord.adapter`.
#
# The `module ActiveRecord ... end` extension at the bottom adds the
# `.adapter` getter/setter that the Ruby source's
# `class << self; attr_accessor :adapter; end` would have produced —
# the runtime_loader transpile pipeline doesn't yet expose
# module-level attr_accessors on the metaclass, so we declare them
# here to keep `ActiveRecord.adapter = X` and `ActiveRecord.adapter.X`
# resolvable.

require "sqlite3"

module Roundhouse
  module Db
    @@db : DB::Database? = nil

    # Per-prepared-statement state: the open ResultSet plus the most
    # recently materialized row. step? advances the cursor and snapshots
    # the row into `current`; column_int/column_text then index into
    # the snapshot. crystal-db's ResultSet is sequential-read-only
    # (`rs.read` consumes one column), so materializing to an array
    # is the way to keep `column_*(stmt, i)` random-access.
    class StmtEntry
      getter result_set : DB::ResultSet
      property current : Array(DB::Any)?

      def initialize(@result_set : DB::ResultSet)
        @current = nil
      end
    end

    @@statements = {} of Int64 => StmtEntry
    @@next_id : Int64 = 0_i64
    @@last_insert_rowid : Int64 = 0_i64
    @@changes : Int64 = 0_i64

    def self.setup_test_db(schema_sql : String) : Nil
      reset_statements
      if old = @@db
        old.close
      end
      db = DB.open("sqlite3::memory:")
      schema_sql.split(";\n").each do |chunk|
        stmt = chunk.strip
        next if stmt.empty?
        db.exec(stmt)
      end
      @@db = db
    end

    def self.conn : DB::Database
      @@db.not_nil!
    end

    def self.open_production_db(path : String, schema_sql : String) : Nil
      reset_statements
      if old = @@db
        old.close
      end
      dir = File.dirname(path)
      Dir.mkdir_p(dir) unless Dir.exists?(dir)
      db = DB.open("sqlite3://#{path}")
      count = db.query_one(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        as: Int64,
      )
      if count == 0
        schema_sql.split(";\n").each do |chunk|
          stmt = chunk.strip
          next if stmt.empty?
          db.exec(stmt)
        end
      end
      @@db = db
    end

    # ── Low-level prepare/step/column API ────────────────────────
    #
    # Mirrors `runtime/spinel/db.rb` and `runtime/typescript/db.ts`
    # verbatim. Model adapter methods (`_adapter_find`, `_adapter_save`,
    # etc.) emitted by `src/lower/model_to_library/adapter_emit.rs`
    # compose inlined SQL via `escape_int`/`escape_string` and dispatch
    # against this surface. Per-statement state lives in the
    # `@@statements` table; opaque Int64 stmt ids index into it.

    # Run any one-shot DDL/INSERT/UPDATE/DELETE. Captures the
    # last_insert_rowid + changes so subsequent calls to those
    # accessors return the most recent values (the same shape as the
    # TS shim: `Db.exec(insert_sql)` followed by `Db.last_insert_rowid`).
    def self.exec(sql : String) : Nil
      result = conn.exec(sql)
      @@last_insert_rowid = result.last_insert_id
      @@changes = result.rows_affected
    end

    # Prepare a SELECT, returning an opaque integer handle. Subsequent
    # `step?` / `column_int` / `column_text` / `finalize` calls take it
    # by reference. Per-process stmt-id sequence; reset across
    # `setup_test_db` / `open_production_db` so test runs start from 1.
    def self.prepare(sql : String) : Int64
      rs = conn.query(sql)
      @@next_id += 1
      @@statements[@@next_id] = StmtEntry.new(rs)
      @@next_id
    end

    # Advance the cursor on a prepared statement. Returns true and
    # snapshots the current row into the stmt entry on success; false
    # (with the snapshot cleared) when the result set is exhausted.
    def self.step?(stmt_id : Int64) : Bool
      entry = @@statements[stmt_id]
      if entry.result_set.move_next
        col_count = entry.result_set.column_count
        row = Array(DB::Any).new(col_count) do
          entry.result_set.read.as(DB::Any)
        end
        entry.current = row
        true
      else
        entry.current = nil
        false
      end
    end

    # Read an integer column at zero-based index from the row most
    # recently snapshotted by `step?`. NULL coerces to 0 (matches the
    # TS shim and `runtime/spinel/db.rb`); non-Int variants of `DB::Any`
    # coerce via `to_i64`.
    def self.column_int(stmt_id : Int64, i : Int64) : Int64
      entry = @@statements[stmt_id]
      row = entry.current.not_nil!
      v = row[i]
      case v
      when Nil     then 0_i64
      when Int64   then v
      when Int32   then v.to_i64
      when Float64 then v.to_i64
      when Float32 then v.to_i64
      when Bool    then v ? 1_i64 : 0_i64
      when String  then v.to_i64? || 0_i64
      else              0_i64
      end
    end

    # Read a text column at zero-based index. NULL coerces to ""
    # (matches the TS shim — lowered code compares strings, never
    # against nil). Bytes/numeric variants stringify via `to_s`.
    def self.column_text(stmt_id : Int64, i : Int64) : String
      entry = @@statements[stmt_id]
      row = entry.current.not_nil!
      v = row[i]
      case v
      when Nil    then ""
      when String then v
      else             v.to_s
      end
    end

    # Release the underlying ResultSet and drop the stmt-table entry.
    # Idempotent — finalize on an unknown stmt id is a no-op (mirrors
    # the TS shim).
    def self.finalize(stmt_id : Int64) : Nil
      entry = @@statements[stmt_id]?
      return if entry.nil?
      entry.result_set.close
      @@statements.delete(stmt_id)
    end

    def self.last_insert_rowid : Int64
      @@last_insert_rowid
    end

    def self.changes : Int64
      @@changes
    end

    # SQL-quote a string value. Single-quotes are doubled per sqlite's
    # string-literal escape rule; no other byte transforms (the lowered
    # adapter emit never inlines binary blobs).
    def self.escape_string(s : String) : String
      "'" + s.gsub("'", "''") + "'"
    end

    # Render an Integer for SQL inlining. Matches the TS shim's
    # truncate-to-int semantics; the Crystal type system already
    # constrains the input to Int, so there's no parse-or-zero
    # fallback.
    def self.escape_int(n : Int) : String
      n.to_s
    end

    # Drain in-flight ResultSets before swapping the underlying
    # connection. Without this, `setup_test_db` between specs would
    # leak ResultSets bound to a closed connection.
    private def self.reset_statements : Nil
      @@statements.each_value do |entry|
        entry.result_set.close rescue nil
      end
      @@statements.clear
      @@next_id = 0_i64
      @@last_insert_rowid = 0_i64
      @@changes = 0_i64
    end
  end

  # Abstract adapter contract — the 9 methods `ActiveRecord::Base`
  # (transpiled from runtime/ruby/active_record/base.rb) calls
  # against `ActiveRecord.adapter`. Every concrete adapter (sqlite
  # production, in-memory framework-test, future libsql/D1) inherits
  # and implements these. Returns are intentionally untyped here —
  # row shape (`Hash(String, DB::Any)` for sqlite, `Hash(String, _)`
  # for the in-memory adapter) varies by implementation, but per-
  # call-site Crystal inference threads the actual concrete type
  # through to `instantiate(row)`.
  #
  # Test-helper methods (`create_table`, `drop_table`, `reset_all!`,
  # `schema`) are NOT in the abstract — they're called directly on
  # `FrameworkTestAdapter` (which has them as concrete methods),
  # never via the `ActiveRecord.adapter` slot.
  abstract class ActiveRecordAdapter
    abstract def all(table_name : String)
    abstract def find(table_name : String, id)
    abstract def where(table_name : String, conditions : Hash(Symbol, _))
    abstract def count(table_name : String) : Int64
    abstract def exists?(table_name : String, id) : Bool
    abstract def insert(table_name : String, attributes : Hash(Symbol, _)) : Int64
    abstract def update(table_name : String, id, attributes : Hash(Symbol, _)) : Nil
    abstract def delete(table_name : String, id) : Nil
    abstract def truncate(table_name : String) : Nil
  end

  # Concrete sqlite-backed adapter. Method names + arities match the
  # Ruby surface; row results come back as `Hash(String, DB::Any)`
  # matching Crystal's crystal-db return shape.
  class SqliteAdapter < ActiveRecordAdapter
    private def conn
      Roundhouse::Db.conn
    end

    def all(table_name : String)
      rows = [] of Hash(String, DB::Any)
      conn.query("SELECT * FROM #{table_name}") do |rs|
        rs.column_count.times { rs.column_name(0) } # warm up metadata
        names = (0...rs.column_count).map { |i| rs.column_name(i) }
        rs.each do
          h = {} of String => DB::Any
          names.each_with_index { |n, i| h[n] = rs.read }
          rows << h
        end
      end
      rows
    end

    def find(table_name : String, id)
      row = nil
      conn.query("SELECT * FROM #{table_name} WHERE id = ? LIMIT 1", id) do |rs|
        names = (0...rs.column_count).map { |i| rs.column_name(i) }
        rs.each do
          h = {} of String => DB::Any
          names.each_with_index { |n, i| h[n] = rs.read }
          row = h
        end
      end
      row
    end

    def where(table_name : String, conditions : Hash(Symbol, _))
      keys = conditions.keys
      rows = [] of Hash(String, DB::Any)
      return rows if keys.empty?
      where_clause = keys.map { |k| "#{k} = ?" }.join(" AND ")
      args = keys.map { |k| conditions[k].as(DB::Any) }
      conn.query("SELECT * FROM #{table_name} WHERE #{where_clause}", args: args) do |rs|
        names = (0...rs.column_count).map { |i| rs.column_name(i) }
        rs.each do
          h = {} of String => DB::Any
          names.each_with_index { |n, i| h[n] = rs.read }
          rows << h
        end
      end
      rows
    end

    def count(table_name : String) : Int64
      conn.query_one("SELECT COUNT(*) FROM #{table_name}", as: Int64)
    end

    def exists?(table_name : String, id) : Bool
      n = conn.query_one(
        "SELECT COUNT(*) FROM #{table_name} WHERE id = ?",
        id,
        as: Int64,
      )
      n > 0
    end

    def insert(table_name : String, attributes : Hash(Symbol, _)) : Int64
      keys = attributes.keys
      cols = keys.map(&.to_s).join(", ")
      placeholders = (["?"] * keys.size).join(", ")
      args = keys.map { |k| attributes[k].as(DB::Any) }
      conn.exec("INSERT INTO #{table_name} (#{cols}) VALUES (#{placeholders})", args: args)
      conn.query_one("SELECT last_insert_rowid()", as: Int64)
    end

    def update(table_name : String, id, attributes : Hash(Symbol, _)) : Nil
      keys = attributes.keys
      return if keys.empty?
      sets = keys.map { |k| "#{k} = ?" }.join(", ")
      args = keys.map { |k| attributes[k].as(DB::Any) } + [id.as(DB::Any)]
      conn.exec("UPDATE #{table_name} SET #{sets} WHERE id = ?", args: args)
    end

    def delete(table_name : String, id) : Nil
      conn.exec("DELETE FROM #{table_name} WHERE id = ?", id)
    end

    def truncate(table_name : String) : Nil
      conn.exec("DELETE FROM #{table_name}")
    end
  end
end

# Module-level attr_accessor analog. The Ruby source declares
# `class << self; attr_accessor :adapter; end` inside `module
# ActiveRecord`; the transpiler doesn't yet emit module-metaclass
# accessors. Re-opening the module here adds the missing surface.
#
# Slot is typed as the abstract base so any adapter implementation
# (production sqlite, framework-test in-memory, future libsql/D1)
# can plug in via `ActiveRecord.adapter = <impl>`.
module ActiveRecord
  @@adapter : Roundhouse::ActiveRecordAdapter? = nil

  def self.adapter : Roundhouse::ActiveRecordAdapter
    @@adapter.not_nil!
  end

  def self.adapter=(value : Roundhouse::ActiveRecordAdapter) : Roundhouse::ActiveRecordAdapter
    @@adapter = value
  end
end
