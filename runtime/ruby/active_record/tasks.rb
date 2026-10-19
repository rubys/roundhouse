# frozen_string_literal: true

# `ActiveRecord::Tasks::DatabaseTasks` for the ruby family and Spinel.
#
# Campfire's counter repair prepends onto this singleton and then runs
# `db:schema:load`, `db:test:load_schema`, `prepare_all`, and `migrate`.
# Those have to switch the process database, load a `schema.rb`, purge
# the file, and rebuild a column — and they have to do it as real method
# calls, because Spinel bakes the ancestor chain at compile time.
#
# The schema reader, the migration runner, and `Rake::Task` live in
# their own files. This one switches the pool and dispatches the task.
#
# `Dir.mktmpdir` and `FileUtils.remove_entry` stay the stdlib (and
# Spinel's packages). This file is on the ActiveRecord boot path, so a
# reopen here would replace the block form those libraries already have.

module ActiveRecord
  def self.dump_schema_after_migration
    value = @dump_schema_after_migration
    value.nil? ? true : value
  end

  def self.dump_schema_after_migration=(value)
    @dump_schema_after_migration = value
    value
  end

  module Tasks
    # The object `seed_loader=` stores. Rails only requires `load_seed`.
    class SeedLoader
      def load_seed
        nil
      end
    end

    class SchemaMigration
      def table_exists?
        ActiveRecord::Base.connection.data_source_exists?("schema_migrations")
      end
    end

    class ConnectionPoolView
      def initialize(db_config)
        @db_config = db_config
      end

      def db_config
        @db_config
      end

      def with_connection
        yield ActiveRecord::Base.connection
        nil
      end

      def schema_migration
        SchemaMigration.new
      end
    end

    module DatabaseTasks
      def self.current_db_config
        return nil unless @config_set == true
        @current
      end

      def self.current_or_process_config
        current = current_db_config
        return current unless current.nil?
        ActiveRecord::DatabaseConfigurations::HashConfig.new(env, "primary", process_database, "sqlite3")
      end

      def self.process_database
        path = @process_database
        if path.nil?
          path = Db.database_path
          @process_database = path
        end
        path
      end

      def self.env
        value = @env
        return value unless value.nil?
        name = Rails.env_name
        name.nil? || name.empty? ? "development" : name
      end

      def self.env=(value)
        @env = value
        value
      end

      def self.migrations_paths
        paths = @migrations_paths
        paths.nil? ? [] : paths
      end

      def self.migrations_paths=(value)
        @migrations_paths = value
        value
      end

      def self.seed_loader
        @seed_loader
      end

      def self.seed_loader=(loader)
        @seed_loader = loader
        loader
      end

      # Spinel cannot represent `Array[HashConfig]` (an array of objects
      # has no unboxed form). An emitted RBS that still claims that shape
      # turns the return into a String at call sites, and
      # `with_temporary_connection` refuses the element. The Spinel tree
      # widens those signatures to `Array[untyped]` in
      # `project::widen_spinel_hash_config_arrays`. Each pushed value is
      # still a HashConfig.
      def self.replace_configs(configs)
        @configs = configs
        nil
      end

      def self.configs_for(env_name)
        list = @configs
        out = []
        return out if list.nil?
        i = 0
        while i < list.length
          config = list[i]
          i += 1
          next if config.nil?
          out << config if config.env_name == env_name
        end
        out
      end

      def self.configs_for_current(env_name)
        configs = configs_for(env_name)
        if env_name == "development" && database_url_absent? && !skip_test_database?
          extra = configs_for("test")
          i = 0
          while i < extra.length
            configs << extra[i]
            i += 1
          end
        end
        configs
      end

      def self.database_url_absent?
        ENV["DATABASE_URL"].nil?
      end

      def self.skip_test_database?
        !ENV["SKIP_TEST_DATABASE"].nil?
      end

      def self.switch_to(db_config)
        if @process_database.nil?
          @process_database = Db.database_path
        end
        path = db_config.database
        if path != Db.database_path
          Db.close
          Db.configure(path)
        end
        @current = db_config
        @config_set = true
        nil
      end

      def self.clear_config
        path = @process_database
        if !path.nil? && path != Db.database_path
          Db.close
          Db.configure(path)
        end
        @current = nil
        @config_set = false
        nil
      end

      def self.with_temporary_pool(db_config)
        previous = @current
        had = @config_set == true
        switch_to(db_config)
        begin
          yield
        ensure
          if had && !previous.nil?
            switch_to(previous)
          else
            clear_config
          end
        end
        nil
      end

      def self.with_temporary_connection(db_config)
        with_temporary_pool(db_config) do
          yield ActiveRecord::Base.connection
          nil
        end
        nil
      end

      def self.purge(db_config)
        path = db_config.database
        # Reopen whenever this method closed the process pool. `@config_set`
        # is about the task's selected config, not about the file Db still
        # has open, so a purge of the live path must configure again even
        # when no task config is current.
        reopened = Db.database_path == path
        Db.close if reopened
        File.delete(path) if File.exist?(path)
        wal = "#{path}-wal"
        shm = "#{path}-shm"
        File.delete(wal) if File.exist?(wal)
        File.delete(shm) if File.exist?(shm)
        Db.configure(path) if reopened
        nil
      end

      def self.purge_test
        configs = configs_for("test")
        i = 0
        while i < configs.length
          purge(configs[i])
          i += 1
        end
        nil
      end

      def self.schema_dump_path(db_config)
        chosen = ENV["SCHEMA"]
        if !chosen.nil? && !chosen.empty?
          return chosen
        end
        dumped = db_config.schema_dump
        return "" if dumped.empty? || dumped == "false"
        return dumped if dumped.start_with?("/")
        File.join("db", dumped)
      end

      def self.load_schema(db_config, format = "", file = "")
        chosen = file
        chosen = schema_dump_path(db_config) if chosen.empty?
        raise "schema file missing for #{db_config.database}" if chosen.empty? || !File.exist?(chosen)
        fmt = format.empty? ? "ruby" : format
        raise "unsupported schema format #{fmt}" if fmt != "ruby"
        load_ruby_schema(File.read(chosen))
        nil
      end

      def self.load_schema_current(format, file)
        configs = configs_for_current(env)
        i = 0
        while i < configs.length
          config = configs[i]
          i += 1
          with_temporary_connection(config) do
            load_schema(config, format, file)
            nil
          end
        end
        nil
      end

      def self.load_test_schemas
        configs = configs_for("test")
        i = 0
        while i < configs.length
          config = configs[i]
          i += 1
          with_temporary_connection(config) do
            load_schema(config, "", "")
            nil
          end
        end
        nil
      end

      def self.initialize_database(db_config)
        return false unless db_config.database_tasks?
        return false if ActiveRecord::Base.connection.data_source_exists?("schema_migrations")
        path = schema_dump_path(db_config)
        return false if path.empty? || !File.exist?(path)
        load_schema(db_config, "", path)
        true
      end

      def self.prepare_all
        configs = configs_for_current(env)
        seed = false
        i = 0
        while i < configs.length
          config = configs[i]
          i += 1
          with_temporary_pool(config) do
            if initialize_database(config) && config.seeds?
              seed = true
            end
            nil
          end
        end
        i = 0
        while i < configs.length
          config = configs[i]
          i += 1
          with_temporary_pool(config) do
            migrate
            nil
          end
        end
        load_seed if seed
        nil
      end

      def self.migrate(version = "")
        config = @current
        initialize_database(config) unless config.nil?
        apply_pending(version)
        nil
      end

      def self.dump_schema_after_migration
        ActiveRecord.dump_schema_after_migration
      end

      def self.check_protected!
        configs = protected_configs
        i = 0
        while i < configs.length
          config = configs[i]
          i += 1
          next if config.nil?
          with_temporary_connection(config) do
            check_open_connection!
            nil
          end
        end
        nil
      end

      # Schema load in development also opens the test database, and
      # `db:test:purge` deletes every test file. The marker has to be
      # read on those files, not only on the connection already open.
      def self.protected_configs
        configs = configs_for_current(env)
        extra = configs_for("test")
        i = 0
        while i < extra.length
          candidate = extra[i]
          i += 1
          next if candidate.nil?
          configs << candidate unless config_listed?(configs, candidate)
        end
        configs
      end

      def self.config_listed?(configs, candidate)
        path = candidate.database
        i = 0
        while i < configs.length
          config = configs[i]
          i += 1
          return true if !config.nil? && config.database == path
        end
        false
      end

      def self.check_open_connection!
        return nil unless ActiveRecord::Base.connection.data_source_exists?("ar_internal_metadata")
        stmt = Db.prepare("SELECT value FROM ar_internal_metadata WHERE key = 'environment'")
        value = ""
        if Db.step?(stmt)
          value = Db.column_text(stmt, 0)
        end
        Db.finalize(stmt)
        if value == "production" && env != "production"
          raise "cannot run database tasks against a production database"
        end
        nil
      end

      def self.load_seed
        loader = @seed_loader
        return nil if loader.nil?
        loader.load_seed
        nil
      end

      def self.migration_connection_pool
        ConnectionPoolView.new(current_or_process_config)
      end

      def self.dispatch(kind)
        if kind == "schema_load"
          load_schema_current("", "")
        elsif kind == "test_purge"
          purge_test
        elsif kind == "test_load_schema"
          load_test_schemas
        elsif kind == "protected"
          check_protected!
        else
          AppRakeTasks.run(kind) unless kind == "noop"
        end
        nil
      end

      def self.load_rake_tasks!
        return nil if @tasks_loaded == true
        @tasks_loaded = true
        define_framework_tasks!
        AppRakeTasks.install!
        nil
      end

      def self.define_framework_tasks!
        Rake::Task.define("environment", [], "noop")
        Rake::Task.define("db:load_config", ["environment"], "noop")
        Rake::Task.define("db:check_protected_environments", ["db:load_config"], "protected")
        Rake::Task.define("db:schema:load", ["db:load_config", "db:check_protected_environments"], "schema_load")
        Rake::Task.define("db:test:purge", ["db:load_config", "db:check_protected_environments"], "test_purge")
        Rake::Task.define("db:test:load_schema", ["db:test:purge"], "test_load_schema")
        nil
      end
    end
  end

  class Connection
    def pool
      ActiveRecord::Tasks::ConnectionPoolView.new(ActiveRecord::Tasks::DatabaseTasks.current_or_process_config)
    end
  end
end

module Rails
  class Application
    def load_tasks
      ActiveRecord::Tasks::DatabaseTasks.load_rake_tasks!
      nil
    end
  end
end

# After DatabaseTasks exists: an app file prepends onto the singleton
# at load, which Spinel only honors for a prepend written in the file
# (not one performed later from a method). The schema reader and the
# migration runner are already loaded, so the prepend sees their methods.
require_relative "app_rake_tasks"
