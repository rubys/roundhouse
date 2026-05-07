# Pure-Ruby adapter satisfying the same contract as SqliteAdapter:
# `all`, `find`, `where`, `count`, `exists?`, `insert`, `update`,
# `delete`, `truncate`, `execute_ddl`. Hash-backed table storage;
# auto-incrementing integer ids per table.
#
# Strategically important for the eventual spinel target — spinel
# has no FFI today, so the C-extension `sqlite3` gem won't compile.
# An in-memory adapter is the path forward there. State is process-
# local: each spinel-binary run starts fresh (acceptable for demos
# + smoke tests; production would need a persistence path that
# spinel can compile, e.g., shell-out-to-sqlite3-CLI).
#
# Spinel-subset compliance: module-level state is held in constants
# (TABLES, NEXT_ID) rather than module @ivars, mirroring the pattern
# used in `runtime/broadcasts.rb`.
module InMemoryAdapter
  module_function

  TABLES  = {}
  NEXT_ID = Hash.new(0)

  def configure(_path = nil)
    TABLES.clear
    NEXT_ID.clear
  end

  def all(table)
    rows_for(table).values
  end

  def find(table, id)
    rows_for(table)[id.to_i]
  end

  def where(table, conditions)
    all(table).select do |row|
      match = true
      conditions.each { |k, v| match = false if row[k.to_s] != v }
      match
    end
  end

  def count(table)
    rows_for(table).size
  end

  def exists?(table, id)
    rows_for(table).key?(id.to_i)
  end

  def insert(table, attrs)
    NEXT_ID[table] += 1
    id = NEXT_ID[table]
    row = { "id" => id }
    attrs.each { |k, v| row[k.to_s] = v }
    rows_for(table)[id] = row
    id
  end

  def update(table, id, attrs)
    row = rows_for(table)[id.to_i]
    return if row.nil?
    attrs.each { |k, v| row[k.to_s] = v }
  end

  def delete(table, id)
    rows_for(table).delete(id.to_i)
  end

  def truncate(table)
    TABLES[table] = {}
    NEXT_ID[table] = 0
  end

  # Schema DDL is mostly cosmetic for in-memory storage — no columns,
  # no constraints, no indexes. Only the table name matters: parsed
  # out so subsequent calls don't error on a missing key. CREATE INDEX
  # statements are silent no-ops.
  def execute_ddl(sql)
    return unless sql =~ /CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(\w+)/i
    TABLES[$1] ||= {}
  end

  def rows_for(name)
    TABLES[name] = {} unless TABLES.key?(name)
    TABLES[name]
  end
end
