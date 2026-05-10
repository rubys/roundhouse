# Test-only adapter mirroring `runtime/ruby/test/test_helper.rb`'s
# `FrameworkTestAdapter` Ruby module and `runtime/typescript/juntos.ts`'s
# `FrameworkTestAdapter` singleton. Framework-level tests under
# `runtime/ruby/test/` reference it directly:
#
#     ::ActiveRecord.adapter = FrameworkTestAdapter
#     FrameworkTestAdapter.reset_all!
#     FrameworkTestAdapter.create_table("stubs", columns: [:id])
#     FrameworkTestAdapter.insert("stubs", id: 7)
#
# Two surface decisions that diverge from the production adapter:
#   1. `create_table` takes a kwargs-shaped `columns:` / `foreign_keys:`
#      pair (the framework runtime's schema.rb DDL helper shape).
#   2. `insert` honors an explicit `id:` in attrs (framework tests
#      pre-assign ids: `insert("stubs", id: 7)`); the production
#      sqlite adapter always autogenerates via `last_insert_rowid()`.
#
# Implemented as a class with instance methods, exposed via the top-
# level `FrameworkTestAdapter` constant pointing at a singleton
# instance. The constant assignment lets tests use it both as a value
# (`adapter = FrameworkTestAdapter`) and as a method receiver
# (`FrameworkTestAdapter.create_table(...)`) — Crystal's lookup
# resolves the constant first, then dispatches the instance method.

module Roundhouse
  # Cell value union covering the types framework tests insert
  # (id integers, title/body strings, optional fields, bools).
  # Wider than `DB::Any` (which excludes Symbol) but narrower than
  # `Object` so the row Hash compiles under strict typing.
  alias TestCellValue = String | Int32 | Int64 | Float64 | Bool | Nil

  alias TestRow = Hash(String, TestCellValue)

  alias TestSchema = NamedTuple(columns: Array(Symbol), foreign_keys: Array(Symbol))

  class FrameworkTestAdapterImpl < ActiveRecordAdapter
    @tables : Hash(String, Hash(Int64, TestRow)) = {} of String => Hash(Int64, TestRow)
    @next_ids : Hash(String, Int64) = {} of String => Int64
    @schemas : Hash(String, TestSchema) = {} of String => TestSchema

    # ── lifecycle / schema (test-helper API; not in abstract base) ───

    def reset_all! : Nil
      @tables.clear
      @next_ids.clear
      @schemas.clear
    end

    def create_table(name : String, *, columns : Array(Symbol), foreign_keys : Array(Symbol) = [] of Symbol) : Nil
      @tables[name] = {} of Int64 => TestRow
      @next_ids[name] = 0_i64
      @schemas[name] = {columns: columns, foreign_keys: foreign_keys}
    end

    def drop_table(name : String) : Nil
      @tables.delete(name)
      @next_ids.delete(name)
      @schemas.delete(name)
    end

    def schema(table : String) : TestSchema?
      @schemas[table]?
    end

    # ── abstract base requirements ───────────────────────────────────

    def all(table_name : String) : Array(TestRow)
      t = @tables[table_name]?
      t ? t.values : ([] of TestRow)
    end

    def find(table_name : String, id) : TestRow?
      t = @tables[table_name]?
      return nil if t.nil?
      t[id.to_i64]?
    end

    def where(table_name : String, conditions : Hash(Symbol, _)) : Array(TestRow)
      all(table_name).select do |row|
        conditions.all? { |k, v| row[k.to_s]? == v }
      end
    end

    def count(table_name : String) : Int64
      t = @tables[table_name]?
      (t ? t.size : 0).to_i64
    end

    def exists?(table_name : String, id) : Bool
      !find(table_name, id).nil?
    end

    def insert(table_name : String, attributes : Hash(Symbol, _)) : Int64
      raise "table #{table_name} not created" unless @tables.has_key?(table_name)
      explicit = attributes[:id]?
      # `to_s.to_i64` rather than `explicit.as(Int).to_i64`: the
      # attributes Hash value type is `_` at the abstract-method
      # boundary, so Crystal can't statically prove explicit is Int
      # even when the caller is passing `id: 7`. The to_s detour
      # handles every cell variant uniformly (Int → "7" → 7,
      # String "7" → 7, nil → "" → 0 — caught by the nil guard
      # above).
      id = if !explicit.nil? && explicit != 0
             explicit.to_s.to_i64
           else
             (@next_ids[table_name]? || 0_i64) + 1_i64
           end
      current = @next_ids[table_name]? || 0_i64
      @next_ids[table_name] = current > id ? current : id
      row = stringify_row(attributes)
      row["id"] = id
      @tables[table_name][id] = row
      id
    end

    def update(table_name : String, id, attributes : Hash(Symbol, _)) : Nil
      t = @tables[table_name]?
      return if t.nil?
      id_i = id.is_a?(Int) ? id.to_i64 : id.to_s.to_i64
      return unless t.has_key?(id_i)
      row = t[id_i].dup
      stringify_row(attributes).each { |k, v| row[k] = v }
      row["id"] = id_i
      t[id_i] = row
    end

    def delete(table_name : String, id) : Nil
      t = @tables[table_name]?
      return if t.nil?
      id_i = id.is_a?(Int) ? id.to_i64 : id.to_s.to_i64
      t.delete(id_i)
    end

    def truncate(table_name : String) : Nil
      @tables[table_name] = {} of Int64 => TestRow
      @next_ids[table_name] = 0_i64
    end

    # ── kwargs convenience overloads for direct test calls ───────────
    #
    # Framework tests use `insert("stubs", id: 7)` (kwargs syntax),
    # which Crystal lifts to NamedTuple at the call site. The
    # abstract-base override above takes `Hash(Symbol, _)` —
    # production callers in transpiled `ActiveRecord::Base` build
    # `attributes` as a Hash, so they hit that overload. These
    # `**attrs` overloads accept the test-shape kwargs, convert to
    # Hash, and forward.

    def insert(table_name : String, **attrs) : Int64
      insert(table_name, namedtuple_to_hash(attrs))
    end

    def update(table_name : String, id, **attrs) : Nil
      update(table_name, id, namedtuple_to_hash(attrs))
    end

    def where(table_name : String, **conditions) : Array(TestRow)
      where(table_name, namedtuple_to_hash(conditions))
    end

    private def namedtuple_to_hash(nt) : Hash(Symbol, TestCellValue)
      h = {} of Symbol => TestCellValue
      nt.each { |k, v| h[k] = v.as(TestCellValue) }
      h
    end

    private def stringify_row(attrs : Hash(Symbol, _)) : TestRow
      h = {} of String => TestCellValue
      attrs.each { |k, v| h[k.to_s] = v.as(TestCellValue) }
      h
    end
  end
end

# Top-level constant — resolves both `ActiveRecord.adapter =
# FrameworkTestAdapter` (value position) and
# `FrameworkTestAdapter.create_table(...)` (instance-method dispatch).
FrameworkTestAdapter = Roundhouse::FrameworkTestAdapterImpl.new
