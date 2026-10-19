# frozen_string_literal: true

# `ActiveRecord::Tasks::DatabaseTasks` for the ruby family and Spinel.
#
# Campfire's counter repair prepends onto this singleton and then runs
# `db:schema:load`, `db:test:load_schema`, `prepare_all`, and `migrate`.
# Those have to switch the process database, load a `schema.rb`, purge
# the file, and rebuild a column — and they have to do it as real method
# calls, because Spinel bakes the ancestor chain at compile time and
# cannot `Kernel#load` a schema someone wrote into a temp file.
#
# The schema and migration readers accept the dump subset those tasks
# write (create_table, change_column) and raise on anything else, so a
# statement we do not run cannot look applied.
#
# `Dir.mktmpdir` and `FileUtils.remove_entry` stay the stdlib (and
# Spinel's packages). This file is on the ActiveRecord boot path, so a
# reopen here would replace the block form those libraries already have.

module Rake
  class Task
    def self.define(name, prerequisites, kind)
      task = Task.new(name, prerequisites, kind)
      task.chain(@head)
      @head = task
      task
    end

    def self.[](name)
      find(name)
    end

    def self.task_defined?(name)
      !find(name).nil?
    end

    def self.find(name)
      current = @head
      while !current.nil?
        return current if current.name == name
        current = current.rest
      end
      nil
    end

    def initialize(name, prerequisites, kind)
      @name = name
      @prerequisites = prerequisites
      @kind = kind
      @invoked = false
      @rest = nil
    end

    def chain(rest)
      @rest = rest
      self
    end

    def rest
      @rest
    end

    def name
      @name
    end

    def reenable
      @invoked = false
      nil
    end

    def all_prerequisite_tasks
      out = []
      deps = @prerequisites
      i = 0
      while i < deps.length
        dep = deps[i].to_s
        i += 1
        pre = Rake::Task[dep]
        next if pre.nil?
        nested = pre.all_prerequisite_tasks
        j = 0
        while j < nested.length
          out << nested[j]
          j += 1
        end
        out << pre
      end
      out
    end

    def invoke
      return nil if @invoked == true
      deps = all_prerequisite_tasks
      i = 0
      while i < deps.length
        deps[i].invoke
        i += 1
      end
      @invoked = true
      ActiveRecord::Tasks::DatabaseTasks.dispatch(@kind)
      nil
    end
  end
end

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

    class MigrationFile
      def initialize(version, path)
        @version = version
        @path = path
      end

      def version
        @version
      end

      def path
        @path
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
        if Db.database_path == path
          Db.close
        end
        File.delete(path) if File.exist?(path)
        wal = "#{path}-wal"
        shm = "#{path}-shm"
        File.delete(wal) if File.exist?(wal)
        File.delete(shm) if File.exist?(shm)
        if @config_set == true && !@current.nil? && @current.database == path
          Db.configure(path)
        end
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
        dumped
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

      def self.load_ruby_schema(text)
        lines = text.split("\n")
        version = ""
        i = 0
        while i < lines.length
          raw = lines[i]
          i += 1
          next if raw.nil?
          line = raw.strip
          next if line.empty? || line.start_with?("#")
          found = version_token(line)
          version = found unless found.empty?
          if line.start_with?("create_table")
            i = read_create_table(lines, i - 1)
            next
          end
          next if line == "end" || line.start_with?("ActiveRecord::Schema") || line.include?(".define")
          raise "unsupported schema statement: #{line}"
        end
        ensure_schema_migrations
        record_version(version)
        nil
      end

      def self.version_token(line)
        marker = "version:"
        at = line.index(marker)
        return "" if at.nil?
        rest = line[at + marker.length, line.length].to_s.strip
        out = ""
        i = 0
        while i < rest.length
          ch = rest[i].to_s
          if ch >= "0" && ch <= "9"
            out += ch
          elsif ch != "_"
            break
          end
          i += 1
        end
        out
      end

      def self.read_create_table(lines, index)
        header = lines[index].to_s.strip
        name = table_name_from(header)
        execute_sql("DROP TABLE IF EXISTS #{quote_ident(name)}") if header.include?("force:")
        cols = []
        cols << "id integer PRIMARY KEY" unless header.include?("id: false")
        index += 1
        while index < lines.length
          line = lines[index].to_s.strip
          index += 1
          next if line.empty? || line.start_with?("#")
          break if line == "end"
          cols << column_def(line)
        end
        execute_sql("CREATE TABLE #{quote_ident(name)} (#{cols.join(", ")})")
        index
      end

      def self.table_name_from(line)
        rest = line[12, line.length].to_s.strip
        token_at(rest)
      end

      def self.column_def(line)
        raise "unsupported schema statement: #{line}" unless line.start_with?("t.")
        rest = line[2, line.length].to_s.strip
        space = rest.index(" ")
        raise "unsupported schema statement: #{line}" if space.nil?
        type_name = rest[0, space].to_s
        name = token_at(rest[space + 1, rest.length].to_s)
        default_sql = ""
        at = line.index("default:")
        if !at.nil?
          raw = line[at + 8, line.length].to_s.strip
          cut = raw.index(",")
          raw = raw[0, cut].to_s.strip unless cut.nil?
          default_sql = " DEFAULT #{literal_sql(raw)}" unless raw.empty? || raw == "nil"
        end
        null_sql = line.include?("null: false") ? " NOT NULL" : ""
        "#{quote_ident(name)} #{sql_type(type_name)}#{default_sql}#{null_sql}"
      end

      def self.token_at(text)
        token = ""
        i = 0
        while i < text.length
          ch = text[i].to_s
          break if ch == " " || ch == "," || ch == ""
          token += ch
          i += 1
        end
        token = token[1, token.length].to_s if token.start_with?(":")
        token = token.gsub("\"", "")
        token.gsub("'", "")
      end

      def self.sql_type(name)
        case name
        when "integer", "bigint", "primary_key"
          "integer"
        when "string"
          "varchar"
        when "text"
          "text"
        when "boolean"
          "boolean"
        when "float", "decimal"
          "float"
        when "binary", "blob"
          "blob"
        when "datetime", "timestamp", "date", "time", "json"
          "datetime"
        else
          raise "unsupported column type #{name}"
        end
      end

      def self.literal_sql(raw)
        if raw == "true"
          "1"
        elsif raw == "false"
          "0"
        elsif raw.start_with?("\"") || raw.start_with?("'")
          raw
        elsif integer_token?(raw)
          raw
        else
          raise "unsupported column default #{raw}"
        end
      end

      def self.integer_token?(raw)
        i = 0
        i = 1 if raw.start_with?("-")
        return false if i >= raw.length
        while i < raw.length
          ch = raw[i].to_s
          return false if ch < "0" || ch > "9"
          i += 1
        end
        true
      end

      def self.quote_ident(text)
        raise "unsupported sql identifier #{text}" unless ident?(text)
        text
      end

      def self.ident?(text)
        return false if text.empty?
        i = 0
        while i < text.length
          ch = text[i].to_s
          letter = (ch >= "a" && ch <= "z") || (ch >= "A" && ch <= "Z") || ch == "_"
          digit = i > 0 && ch >= "0" && ch <= "9"
          return false unless letter || digit
          i += 1
        end
        true
      end

      def self.execute_sql(sql)
        ActiveRecord::Base.connection.execute(sql)
        nil
      end

      def self.ensure_schema_migrations
        return nil if ActiveRecord::Base.connection.data_source_exists?("schema_migrations")
        execute_sql("CREATE TABLE schema_migrations (version varchar NOT NULL PRIMARY KEY)")
        nil
      end

      def self.record_version(version)
        return nil if version.empty?
        quoted = ActiveRecord::Base.connection.quote(version)
        execute_sql("INSERT OR IGNORE INTO schema_migrations (version) VALUES (#{quoted})")
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

      def self.apply_pending(target)
        ensure_schema_migrations
        files = migration_files
        ran = false
        i = 0
        while i < files.length
          file = files[i]
          i += 1
          next if file.nil?
          version = file.version
          next if !target.empty? && version > target
          next if version_recorded?(version)
          apply_migration(File.read(file.path))
          record_version(version)
          ran = true
        end
        if ran && dump_schema_after_migration
          raise "schema dump after migration is not supported"
        end
        nil
      end

      def self.dump_schema_after_migration
        ActiveRecord.dump_schema_after_migration
      end

      def self.version_recorded?(version)
        quoted = ActiveRecord::Base.connection.quote(version)
        stmt = Db.prepare("SELECT version FROM schema_migrations WHERE version = #{quoted}")
        found = Db.step?(stmt)
        Db.finalize(stmt)
        found
      end

      def self.migration_files
        dirs = migrations_paths
        found = []
        i = 0
        while i < dirs.length
          dir = dirs[i].to_s
          i += 1
          next unless File.directory?(dir)
          names = Dir.entries(dir)
          j = 0
          while j < names.length
            name = names[j].to_s
            j += 1
            next unless name.end_with?(".rb")
            version = leading_digits(name)
            next if version.empty?
            found << MigrationFile.new(version, File.join(dir, name))
          end
        end
        sort_migrations(found)
      end

      def self.leading_digits(name)
        out = ""
        i = 0
        while i < name.length
          ch = name[i].to_s
          break if ch < "0" || ch > "9"
          out += ch
          i += 1
        end
        out
      end

      def self.sort_migrations(files)
        i = 1
        while i < files.length
          current = files[i]
          j = i
          while j > 0 && !files[j - 1].nil? && !current.nil? && files[j - 1].version > current.version
            files[j] = files[j - 1]
            j -= 1
          end
          files[j] = current
          i += 1
        end
        files
      end

      def self.apply_migration(text)
        lines = text.split("\n")
        applying = false
        method_indent = -1
        i = 0
        while i < lines.length
          raw = lines[i]
          i += 1
          next if raw.nil?
          indent = leading_spaces(raw)
          line = raw.strip
          next if line.empty? || line.start_with?("#")
          if line.start_with?("def ")
            name = method_name(line)
            applying = name == "change" || name == "up"
            method_indent = indent
            next
          end
          if applying && indent <= method_indent && (line == "end" || line.start_with?("end "))
            applying = false
            next
          end
          next if line.start_with?("class ") || line.start_with?("module ") || line == "end" || line.start_with?("end ")
          next unless applying
          if line.start_with?("change_column")
            apply_change_column(line)
          else
            raise "unsupported migration statement: #{line}"
          end
        end
        nil
      end

      def self.leading_spaces(raw)
        n = 0
        while n < raw.length && raw[n].to_s == " "
          n += 1
        end
        n
      end

      def self.method_name(line)
        rest = line[4, line.length].to_s.strip
        paren = rest.index("(")
        rest = rest[0, paren].to_s.strip unless paren.nil?
        token_at(rest)
      end

      def self.apply_change_column(line)
        parts = parts_of(line[13, line.length].to_s)
        raise "unsupported migration statement: #{line}" if parts.length < 3
        table = parts[0].to_s
        column = parts[1].to_s
        type_name = parts[2].to_s
        rebuild_column(table, column, type_name)
        nil
      end

      def self.parts_of(text)
        parts = []
        rest = text.strip
        while !rest.empty?
          comma = rest.index(",")
          piece = comma.nil? ? rest : rest[0, comma].to_s
          parts << clean_token(piece)
          if comma.nil?
            rest = ""
          else
            rest = rest[comma + 1, rest.length].to_s.strip
          end
        end
        parts
      end

      def self.clean_token(piece)
        token = piece.strip
        token = token[1, token.length].to_s if token.start_with?(":")
        token = token.gsub("\"", "")
        token.gsub("'", "")
      end

      def self.rebuild_column(table, column, type_name)
        quoted = quote_ident(table)
        stmt = Db.prepare("PRAGMA table_info(#{quoted})")
        names = []
        types = []
        notnulls = []
        defaults = []
        pks = []
        while Db.step?(stmt)
          name = Db.column_text(stmt, 1)
          names << name
          if name == column
            types << sql_type(type_name)
          else
            types << Db.column_text(stmt, 2)
          end
          notnulls << Db.column_int(stmt, 3)
          defaults << Db.column_text(stmt, 4)
          pks << Db.column_int(stmt, 5)
        end
        Db.finalize(stmt)
        raise "no such table #{table}" if names.empty?
        raise "no such column #{column}" unless names.include?(column)
        refuse_unpreserved(table)
        defs = []
        selected = []
        i = 0
        while i < names.length
          piece = quote_ident(names[i].to_s)
          dflt = defaults[i].to_s
          default_sql = dflt.empty? ? "" : " DEFAULT #{dflt}"
          null_sql = notnulls[i] == 1 ? " NOT NULL" : ""
          pk_sql = pks[i] == 1 ? " PRIMARY KEY" : ""
          defs << "#{piece} #{types[i]}#{default_sql}#{null_sql}#{pk_sql}"
          selected << piece
          i += 1
        end
        tmp = quote_ident("#{table}__rh_new")
        joined = selected.join(", ")
        execute_sql("CREATE TABLE #{tmp} (#{defs.join(", ")})")
        execute_sql("INSERT INTO #{tmp} (#{joined}) SELECT #{joined} FROM #{quoted}")
        execute_sql("DROP TABLE #{quoted}")
        execute_sql("ALTER TABLE #{tmp} RENAME TO #{quoted}")
        nil
      end

      # SQLite's table rewrite drops indexes and foreign keys. Rails copies
      # them back. Recording the version without that copy would mark a
      # migration applied after throwing those away. Triggers are dropped
      # on purpose: Rails does not put them back, and the prepended hook
      # is what repairs the ones the app owns.
      def self.refuse_unpreserved(table)
        quoted = ActiveRecord::Base.connection.quote(table)
        stmt = Db.prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = #{quoted}")
        while Db.step?(stmt)
          name = Db.column_text(stmt, 0)
          next if name.start_with?("sqlite_")
          Db.finalize(stmt)
          raise "change_column cannot preserve index #{name}"
        end
        Db.finalize(stmt)
        listed = Db.prepare("PRAGMA foreign_key_list(#{quote_ident(table)})")
        if Db.step?(listed)
          Db.finalize(listed)
          raise "change_column cannot preserve foreign keys on #{table}"
        end
        Db.finalize(listed)
        nil
      end

      def self.check_protected!
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
# (not one performed later from a method).
require_relative "app_rake_tasks"
