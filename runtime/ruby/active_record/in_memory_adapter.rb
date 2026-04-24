module ActiveRecord
  # Test-time storage backing for ActiveRecord::Base. Each target's
  # primitive runtime provides its own adapter (SQLite, IndexedDB, etc.).
  # This CRuby adapter lets framework Ruby be tested directly.
  class InMemoryAdapter
    def initialize
      @tables = {}
      @next_id = Hash.new(0)
      @schemas = {}
    end

    def create_table(name, columns:, foreign_keys: [])
      name = name.to_s
      @tables[name] = {}
      @schemas[name] = { columns: columns, foreign_keys: foreign_keys }
    end

    def drop_table(name)
      name = name.to_s
      @tables.delete(name)
      @schemas.delete(name)
      @next_id.delete(name)
    end

    def schema(table)
      @schemas[table.to_s]
    end

    def insert(table, row)
      name = table.to_s
      @next_id[name] += 1
      id = @next_id[name]
      row = row.merge(id: id)
      @tables[name][id] = row
      id
    end

    def update(table, id, row)
      name = table.to_s
      return false unless @tables[name] && @tables[name][id]
      @tables[name][id] = row.merge(id: id)
      true
    end

    def delete(table, id)
      name = table.to_s
      return false unless @tables[name]
      @tables[name].delete(id) ? true : false
    end

    def find(table, id)
      return nil unless @tables[table.to_s]
      @tables[table.to_s][id]
    end

    def all(table)
      return [] unless @tables[table.to_s]
      @tables[table.to_s].values
    end

    def where(table, conditions)
      all(table).select do |row|
        conditions.all? { |k, v| row[k.to_sym] == v }
      end
    end

    def count(table)
      return 0 unless @tables[table.to_s]
      @tables[table.to_s].size
    end

    def exists?(table, id)
      return false unless @tables[table.to_s]
      @tables[table.to_s].key?(id)
    end
  end
end
