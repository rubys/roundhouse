# Roundhouse Crystal DB runtime — sqlite primitive layer plus the
# `ActiveRecord.adapter` plug-in.
#
# Two responsibilities:
#   1. `Roundhouse::Db` — owns the sqlite3 connection. `open_production_db`
#      is called from `Roundhouse::Server.start`; `setup_test_db` resets
#      the connection between specs.
#   2. `Roundhouse::SqliteAdapter` — implements the abstract API the
#      transpiled `ActiveRecord::Base` calls (`all`, `find`, `where`,
#      `insert`, `update`, `delete`, `count`, `exists?`). Server boot
#      assigns an instance to `ActiveRecord.adapter`.
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

    def self.setup_test_db(schema_sql : String) : Nil
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
  end

  # Concrete sqlite-backed adapter implementing the API
  # `ActiveRecord::Base` (transpiled from runtime/ruby/active_record/
  # base.rb) calls. Method names + arities match the Ruby surface;
  # row results come back as `Hash(String, DB::Any)` matching Crystal's
  # crystal-db return shape.
  class SqliteAdapter
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
  end
end

# Module-level attr_accessor analog. The Ruby source declares
# `class << self; attr_accessor :adapter; end` inside `module
# ActiveRecord`; the transpiler doesn't yet emit module-metaclass
# accessors. Re-opening the module here adds the missing surface.
module ActiveRecord
  @@adapter : Roundhouse::SqliteAdapter? = nil

  def self.adapter : Roundhouse::SqliteAdapter
    @@adapter.not_nil!
  end

  def self.adapter=(value : Roundhouse::SqliteAdapter) : Roundhouse::SqliteAdapter
    @@adapter = value
  end
end
