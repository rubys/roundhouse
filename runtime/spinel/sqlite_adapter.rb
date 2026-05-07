require "sqlite3"

# Gem-backed sqlite adapter. Implements the contract that AR base relies
# on: take a table name + plain values, return plain row hashes (Symbol
# keys → primitive values). Designed so a future shell-out-to-`sqlite3`
# adapter (under Spinel, where there's no FFI) can drop in 1:1 — no FFI
# handles surface in the API; no prepared-statement caching is exposed;
# no connection-pool concept leaks through.
module SqliteAdapter
  module_function

  @db = nil

  def configure(database_path)
    @db = SQLite3::Database.new(database_path)
    @db.results_as_hash = true
  end

  def db
    raise "SqliteAdapter not configured; call SqliteAdapter.configure(path) first" if @db.nil?
    @db
  end

  def execute(sql, params = [])
    rows = db.execute(sql, params)
    rows.map { |row| symbolize_row(row) }
  end

  def all(table)
    execute("SELECT * FROM #{table}")
  end

  def find(table, id)
    rows = execute("SELECT * FROM #{table} WHERE id = ? LIMIT 1", [id])
    rows.first
  end

  def where(table, conditions)
    where_clause, values = build_where(conditions)
    sql = "SELECT * FROM #{table}"
    sql += " WHERE #{where_clause}" unless where_clause.empty?
    execute(sql, values)
  end

  def count(table)
    rows = db.execute("SELECT COUNT(*) AS n FROM #{table}")
    rows.first["n"].to_i
  end

  def exists?(table, id)
    rows = db.execute("SELECT 1 FROM #{table} WHERE id = ? LIMIT 1", [id])
    !rows.empty?
  end

  def insert(table, attrs)
    cols = attrs.keys
    placeholders = cols.map { "?" }.join(", ")
    sql = "INSERT INTO #{table} (#{cols.join(", ")}) VALUES (#{placeholders})"
    db.execute(sql, attrs.values)
    db.last_insert_row_id
  end

  def update(table, id, attrs)
    assigns = attrs.keys.map { |k| "#{k} = ?" }.join(", ")
    sql = "UPDATE #{table} SET #{assigns} WHERE id = ?"
    db.execute(sql, attrs.values + [id])
  end

  def delete(table, id)
    db.execute("DELETE FROM #{table} WHERE id = ?", [id])
  end

  # Adapter-agnostic table reset (test setup). Issues both the row
  # delete and the autoincrement-counter reset so subsequent inserts
  # start from id=1 — InMemoryAdapter#truncate has the same effect
  # by clearing its NEXT_ID hash.
  def truncate(table)
    db.execute("DELETE FROM #{table}")
    db.execute("DELETE FROM sqlite_sequence WHERE name = ?", [table])
  end

  def execute_ddl(sql)
    db.execute_batch(sql)
  end

  # Internal: SQLite3::Database with results_as_hash=true returns rows
  # carrying both string keys and integer indexes; strip to string-keyed
  # hashes before handing back. (Was symbol-keyed; switched to String
  # to match the Crystal/TS adapter shape — Crystal can't dynamically
  # create Symbols at runtime, and the IR's `<Model>Row.from_raw`
  # boundary now uses String keys uniformly.)
  def self.symbolize_row(row)
    out = {}
    row.each do |key, val|
      out[key] = val if key.is_a?(String)
    end
    out
  end

  def self.build_where(conditions)
    return ["", []] if conditions.empty?
    clauses = []
    values = []
    conditions.each do |key, val|
      clauses << "#{key} = ?"
      values << val
    end
    [clauses.join(" AND "), values]
  end
end
