# Generic ActiveRecord adapter facade routed through the `Db.*` primitive
# surface. Implements the 10-method contract that AR Base's `_adapter_*`
# defaults call into when a model hasn't received the lowerer's Level-3
# emit (real-blog models all do; this code path is a documented fallback).
#
# Same source compiles under both Db variants:
#   - `runtime/db.rb` = `db_cruby.rb` (gem-backed) under CRuby
#   - `runtime/db.rb` = `db.rb`       (FFI-backed) under spinel-AOT
# No SQLite3 / FFI references appear here directly; everything goes
# through Db.exec / Db.prepare / Db.column_*.
#
# SQL composition uses Db.escape_string / Db.escape_int rather than
# placeholder bind params — the FFI shim can't construct SQLITE_TRANSIENT
# for bind_text, so the shared contract is "inline escaped values".
module SqliteAdapter
  def self.configure(database_path)
    Db.configure(database_path)
  end

  def self.execute_ddl(sql)
    Db.exec(sql)
  end

  # Rows affected by the last INSERT/UPDATE/DELETE — Rails'
  # `delete_all`/`update_all` return this count.
  def self.changes
    Db.changes
  end

  def self.all(table)
    select_rows("SELECT * FROM #{table}")
  end

  def self.find(table, id)
    rows = select_rows("SELECT * FROM #{table} WHERE id = #{Db.escape_int(id)} LIMIT 1")
    rows.empty? ? nil : rows[0]
  end

  def self.where(table, conditions)
    sql = "SELECT * FROM #{table}"
    sql += " WHERE #{build_where(conditions)}" unless conditions.empty?
    select_rows(sql)
  end

  def self.count(table)
    stmt = Db.prepare("SELECT COUNT(*) FROM #{table}")
    n = Db.step?(stmt) ? Db.column_int(stmt, 0) : 0
    Db.finalize(stmt)
    n
  end

  def self.exists?(table, id)
    stmt = Db.prepare("SELECT 1 FROM #{table} WHERE id = #{Db.escape_int(id)} LIMIT 1")
    found = Db.step?(stmt)
    Db.finalize(stmt)
    found
  end

  def self.insert(table, attrs)
    cols = attrs.keys
    values = cols.map { |k| escape_value(attrs[k]) }
    sql = "INSERT INTO #{table} (#{cols.join(", ")}) VALUES (#{values.join(", ")})"
    Db.exec(sql)
    Db.last_insert_rowid
  end

  def self.update(table, id, attrs)
    assigns = attrs.keys.map { |k| "#{k} = #{escape_value(attrs[k])}" }
    sql = "UPDATE #{table} SET #{assigns.join(", ")} WHERE id = #{Db.escape_int(id)}"
    Db.exec(sql)
  end

  def self.delete(table, id)
    Db.exec("DELETE FROM #{table} WHERE id = #{Db.escape_int(id)}")
  end

  # `Model.delete_all` — bulk row delete with ActiveRecord semantics:
  # rows go, the autoincrement counter stays (unlike `truncate`).
  def self.delete_all(table)
    Db.exec("DELETE FROM #{table}")
  end

  # Adapter-agnostic table reset (test setup). Issues both the row
  # delete and the autoincrement-counter reset so subsequent inserts
  # start from id=1.
  def self.truncate(table)
    Db.exec("DELETE FROM #{table}")
    Db.exec("DELETE FROM sqlite_sequence WHERE name = #{Db.escape_string(table)}")
  end

  # Internal helpers — walk a prepared statement and build a row hash
  # keyed by column name. Values read through `Db.column_value` — the
  # driver's NATIVE types (Integer/Float/String, nil for NULL), which
  # is what ActiveRecord hands the app: `group_by(&:fk)[nil]` finds
  # root rows, integer columns compare as integers. (The FFI Db shim
  # still returns text values, nil-for-NULL aside — see db.rb.) Each
  # row is its own Hash so the caller gets a stable copy after
  # `finalize`.
  def self.select_rows(sql)
    stmt = Db.prepare(sql)
    rows = []
    ncols = Db.column_count(stmt)
    # Column names can't change between rows — resolve them once, not
    # rows×cols times (each column_name is a shim call, and hydrating
    # a 1000-row result was paying 24,000 of them).
    names = []
    i = 0
    while i < ncols
      names.push(Db.column_name(stmt, i))
      i += 1
    end
    while Db.step?(stmt)
      row = {}
      i = 0
      while i < ncols
        row[names[i]] = Db.column_value(stmt, i)
        i += 1
      end
      rows << row
    end
    Db.finalize(stmt)
    rows
  end

  def self.build_where(conditions)
    conditions.map { |k, v| "#{k} = #{escape_value(v)}" }.join(" AND ")
  end

  def self.escape_value(v)
    if v.is_a?(Integer)
      Db.escape_int(v)
    elsif v == true
      "1"
    elsif v == false
      "0"
    elsif v.nil?
      "NULL"
    else
      Db.escape_string(v.to_s)
    end
  end
end
