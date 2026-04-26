module ActiveRecord
  # The 12-method shim API that every adapter implementation provides.
  # Framework Ruby calls into this surface for all DB access; the
  # concrete implementations are per-target boundary code (CRuby's
  # InMemoryAdapter for tests, juntos.ts's InMemoryActiveRecordAdapter
  # for TS, sqlx-backed for Rust, etc.) and live outside this tree.
  #
  # `AbstractAdapter` exists to give the framework a typed reference
  # without committing to a specific implementation. Every method
  # raises `NotImplementedError` — installing an adapter via
  # `ActiveRecord.adapter = ...` is mandatory before use.
  class AbstractAdapter
    def create_table(_name, columns:, foreign_keys: [])
      raise NotImplementedError, "#{self.class}#create_table"
    end

    def drop_table(_name)
      raise NotImplementedError, "#{self.class}#drop_table"
    end

    def schema(_table)
      raise NotImplementedError, "#{self.class}#schema"
    end

    def insert(_table, _row)
      raise NotImplementedError, "#{self.class}#insert"
    end

    def update(_table, _id, _row)
      raise NotImplementedError, "#{self.class}#update"
    end

    def delete(_table, _id)
      raise NotImplementedError, "#{self.class}#delete"
    end

    def find(_table, _id)
      raise NotImplementedError, "#{self.class}#find"
    end

    def all(_table)
      raise NotImplementedError, "#{self.class}#all"
    end

    def where(_table, _conditions)
      raise NotImplementedError, "#{self.class}#where"
    end

    def count(_table)
      raise NotImplementedError, "#{self.class}#count"
    end

    def exists?(_table, _id)
      raise NotImplementedError, "#{self.class}#exists?"
    end
  end
end
