# Raw-SQL surface of `Model.connection` / `ActiveRecord::Base.connection`.
#
# Rails hands back the adapter itself here; this runtime hands back a
# thin stateless facade over the per-target `Db` primitive shim — just
# the members the corpus reaches for when it drops below the Relation
# layer (lobsters' Keystore upserts, hand-written aggregate queries,
# `quote`/`quote_string` in SQL-building helpers). Statically
# resolvable by construction: fixed methods, no method_missing.
#
# Result rows are `Hash[String, untyped]` — raw SQL is the one place
# the row shape is genuinely dynamic (aliased aggregates, computed
# columns), so a typed bag is the honest contract rather than an
# avoidable erasure.
module ActiveRecord
  # Row set from `Connection#execute` / `#exec_query`. Mirrors the
  # slice of `ActiveRecord::Result` the corpus uses: `to_a`, `first`,
  # `each`, `rows`.
  class Result
    def initialize(rows)
      @rows = rows
    end

    def rows
      @rows
    end

    def to_a
      @rows
    end

    def first
      @rows.first
    end

    def each
      @rows.each do |row|
        yield row
      end
      @rows
    end
  end

  class Connection
    # This runtime's only backend. Lobsters branches on this to pick
    # its upsert dialect; the SQLite arm is the one we execute.
    def adapter_name
      "SQLite"
    end

    # Rails `quote`: a full SQL literal, quotes included for strings.
    # `Db.escape_string` already wraps in single quotes (sqlite literal
    # syntax with '' doubling).
    def quote(value)
      if value.nil?
        "NULL"
      elsif value.is_a?(Integer) || value.is_a?(Float)
        value.to_s
      elsif value.is_a?(TrueClass)
        "1"
      elsif value.is_a?(FalseClass)
        "0"
      else
        Db.escape_string(value.to_s)
      end
    end

    # Rails `quote_string`: escaped but UNquoted (callers embed it
    # inside their own quotes).
    def quote_string(str)
      str.gsub("'", "''")
    end

    # Run raw SQL, collecting every row as name→value. DML statements
    # simply produce zero rows. One prepare/step/finalize cycle — the
    # same protocol the lowerer-emitted `_adapter_*` methods use.
    def execute(sql)
      stmt = Db.prepare(sql)
      rows = []
      while Db.step?(stmt)
        row = {}
        i = 0
        n = Db.column_count(stmt)
        while i < n
          row[Db.column_name(stmt, i)] = Db.column_value(stmt, i)
          i += 1
        end
        rows.push(row)
      end
      Db.finalize(stmt)
      Result.new(rows)
    end

    def exec_query(sql)
      execute(sql)
    end
  end

  # The Base half of the raw-SQL surface. Lives HERE (not base.rb)
  # deliberately: base.rb is transpiled into every strict target's
  # runtime via the runtime_loader tables, and this surface uses
  # begin/rescue (which several emitters don't lower yet) and the
  # Connection class (which those tables don't ship). This file is
  # walked only into the ruby-family trees, and active_record.rb
  # requires it AFTER base.rb so the reopen sees the real class.
  class Base
    # Stateless facade — every member delegates straight to `Db`, so a
    # fresh instance per call is cheap and dodges class-ivar state.
    def self.connection
      ActiveRecord::Connection.new
    end

    # `Model.transaction { ... }` — the block inside BEGIN/COMMIT, with
    # ROLLBACK + re-raise on any exception. Flat transactions only: the
    # corpus never nests (a nested BEGIN would error in SQLite rather
    # than silently join, which is the honest failure).
    def self.transaction
      Db.exec("BEGIN")
      begin
        result = yield
        Db.exec("COMMIT")
        result
      rescue => e
        Db.exec("ROLLBACK")
        raise e
      end
    end
  end
end
