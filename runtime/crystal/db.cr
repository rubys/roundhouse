# Roundhouse Crystal DB runtime.
#
# Hand-written helpers the Crystal emitter copies verbatim into each
# generated project as `src/db.cr`. Owns the SQLite connection and
# hides `crystal-db`'s `DB::Database` API from the generated code —
# save/destroy/count/find all reach the connection via
# `Roundhouse::Db.conn`.
#
# Crystal spec runs sequentially by default, so a single module-level
# connection is safe; `setup_test_db` is called from `Spec.before_each`
# to reset it between tests. (A fiber-local slot would generalize to
# parallel specs later.)

require "sqlite3"

module Roundhouse
  module Db
    @@db : DB::Database? = nil

    # Open a fresh :memory: SQLite connection, run the schema DDL,
    # and install it in the thread-local slot. Called by
    # `Fixtures.setup` at the top of every spec.
    #
    # The schema string may contain several `CREATE TABLE` statements;
    # `DB::Database#exec` accepts one statement at a time, so we split
    # on `;\n` and dispatch each non-empty chunk.
    def self.setup_test_db(schema_sql : String)
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

    # Borrow the current connection. Raises if `setup_test_db` hasn't
    # been called yet — that would only happen if a generated test
    # bypassed the spec harness.
    def self.conn : DB::Database
      @@db.not_nil!
    end
  end
end
