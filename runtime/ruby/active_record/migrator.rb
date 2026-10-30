# frozen_string_literal: true
# Rails' `db:migrate` and `db:prepare` for an emitted app, over the
# adapter seam (`ActiveRecord.adapter`).
#
# The app's migrations arrive as `Migrations.all` (config/migrations.rb,
# lowered from db/migrate): one Array per migration, in version order:
# its version, then "" or why it cannot run here, then the SQLite
# statements Rails' adapter would issue for it. `Schema.statements` is the
# app's schema, which `prepare` loads into a database that was never set
# up, as Rails' `db:prepare` loads schema.rb; `Migrations.schema_versions`
# are the migrations it already includes (schema.rb's `version:` and
# older), recorded as applied when it is loaded.
#
# As in Rails: `schema_migrations` lists the versions applied; each
# pending migration runs in its own transaction and records its version
# when it succeeds; a failure rolls that migration back and stops.
# SQLite-only, like the rest of the emitted DDL.

module ActiveRecord
  # A migration that cannot run, or a database whose migration state
  # cannot be told (Rails' ActiveRecord::MigrationError family).
  class MigrationError < StandardError
  end

  module Migrator
    SCHEMA_MIGRATIONS_DDL = "CREATE TABLE IF NOT EXISTS \"schema_migrations\" (\"version\" varchar NOT NULL PRIMARY KEY)"
    INTERNAL_METADATA_DDL = "CREATE TABLE IF NOT EXISTS \"ar_internal_metadata\" (\"key\" varchar NOT NULL PRIMARY KEY, \"value\" varchar, \"created_at\" datetime(6) NOT NULL, \"updated_at\" datetime(6) NOT NULL)"

    # Does the database track migrations (Rails or `prepare` set it up)?
    def self.set_up?
      rows = ActiveRecord.adapter.select_rows("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'")
      !rows.empty?
    end

    # Does the database hold any table at all?
    def self.any_tables?
      rows = ActiveRecord.adapter.select_rows("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
      !rows.empty?
    end

    # The versions `schema_migrations` lists.
    def self.applied_versions
      ActiveRecord.adapter.select_rows("SELECT version FROM schema_migrations").map { |row| row["version"].to_s }
    end

    # The migrations a set-up database has not run, in version order.
    def self.pending(migrations)
      applied = applied_versions
      migrations.select { |m| !applied.include?(m[0]) }
    end

    # Run every pending migration. Answers how many ran.
    def self.migrate(migrations, environment)
      if !set_up?
        raise MigrationError, "the database does not track migrations (no schema_migrations table); db:prepare sets up a new one"
      end
      ran = 0
      pending(migrations).each do |m|
        version = m[0]
        gap = m[1]
        if !gap.empty?
          raise MigrationError, "migration " + version + " cannot run here: " + gap
        end
        begin
          run_in_transaction(m.drop(2) + ["INSERT INTO \"schema_migrations\" (\"version\") VALUES ('" + version + "')"])
        rescue MigrationError => e
          raise e
        rescue StandardError => e
          # as Rails reports it: which migration, and that it and the ones
          # after it did not run
          raise MigrationError, "migration " + version + " failed and was rolled back; it and all later migrations were canceled: " + e.to_s
        end
        ran += 1
      end
      record_environment(environment)
      ran
    end

    # Rails' db:prepare: a database that was never set up gets the schema,
    # with the versions it includes recorded, and then the migrations
    # newer than it; one that was set up gets migrated. Answers how many
    # migrations ran.
    def self.prepare(schema_statements, schema_versions, migrations, environment)
      if set_up?
        return migrate(migrations, environment)
      end
      if any_tables?
        raise MigrationError, "the database has tables but no schema_migrations table, so which migrations it has is unknown; it predates migration tracking"
      end
      load_schema(schema_statements, schema_versions, environment)
      migrate(migrations, environment)
    end

    # What the server does at boot: set up a database that was never set
    # up (the schema, as `prepare` loads it; migrations newer than the
    # schema are left pending); leave a set-up one alone (Rails' server does not migrate) and
    # answer the versions still pending, for the caller to report. A
    # database an earlier build created (tables, no schema_migrations)
    # keeps that build's behaviour: the schema's tables are created if
    # missing, and nothing is recorded.
    def self.boot(schema_statements, schema_versions, migrations, environment)
      if set_up?
        return pending(migrations).map { |m| m[0] }
      end
      if any_tables?
        schema_statements.each { |sql| ActiveRecord.adapter.execute_ddl(sql) }
        return []
      end
      load_schema(schema_statements, schema_versions, environment)
      pending(migrations).map { |m| m[0] }
    end

    # The schema plus the versions it includes, in one transaction.
    def self.load_schema(schema_statements, schema_versions, environment)
      inserts = schema_versions.map { |v| "INSERT INTO \"schema_migrations\" (\"version\") VALUES ('" + v + "')" }
      run_in_transaction(schema_statements + [SCHEMA_MIGRATIONS_DDL, INTERNAL_METADATA_DDL] + inserts)
      record_environment(environment)
    end

    # `ar_internal_metadata`'s environment row, as Rails keeps it.
    def self.record_environment(environment)
      ActiveRecord.adapter.execute_ddl(INTERNAL_METADATA_DDL)
      ActiveRecord.adapter.execute_ddl("INSERT INTO \"ar_internal_metadata\" (\"key\", \"value\", \"created_at\", \"updated_at\") VALUES ('environment', " + ActiveRecord.adapter.escape_value(environment) + ", CURRENT_TIMESTAMP, CURRENT_TIMESTAMP) ON CONFLICT(\"key\") DO UPDATE SET \"value\" = excluded.\"value\", \"updated_at\" = CURRENT_TIMESTAMP")
    end

    # The statements in one transaction; any failure rolls them all back.
    def self.run_in_transaction(statements)
      ActiveRecord.adapter.execute_ddl("BEGIN")
      begin
        statements.each { |sql| ActiveRecord.adapter.execute_ddl(sql) }
        ActiveRecord.adapter.execute_ddl("COMMIT")
      rescue StandardError => e
        ActiveRecord.adapter.execute_ddl("ROLLBACK")
        raise e
      end
    end
  end
end
