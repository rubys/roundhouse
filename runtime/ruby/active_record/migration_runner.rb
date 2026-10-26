# frozen_string_literal: true

# Pending files under `migrations_paths`, and the one SQLite rewrite
# this runtime runs (`change_column` by rebuilding the table). A
# statement that is not that rewrite raises before its version is
# recorded. Schema load marks every file at or below the dump version
# as already run, so `prepare_all`'s later `migrate` does not replay
# them.
module ActiveRecord
  module Tasks
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

    module DatabaseTasks
      def self.assume_migrated_upto_version(version)
        return nil if version.empty?
        files = migration_files
        i = 0
        while i < files.length
          file = files[i]
          i += 1
          next if file.nil?
          record_version(file.version) if version_at_most?(file.version, version)
        end
        nil
      end

      # Digit strings, compared as integers. `"9"` is less than `"10"`.
      # A leading zero does not make a version longer.
      def self.version_at_most?(version, limit)
        return false if version.empty? || limit.empty?
        left = numeric_version(version)
        right = numeric_version(limit)
        return true if left.length < right.length
        return false if left.length > right.length
        left <= right
      end

      def self.numeric_version(version)
        i = 0
        while i < version.length - 1 && version[i].to_s == "0"
          i += 1
        end
        version[i, version.length].to_s
      end

      def self.apply_pending(target)
        files = migration_files
        # Refuse before any write. Creating schema_migrations first would
        # make initialize_database skip the dump, and the raise would
        # leave that empty table behind. A rewrite would also drop
        # triggers before `super.tap { repair }` runs.
        if pending?(files, target)
          if dump_schema_after_migration
            raise "schema dump after migration is not supported"
          end
          ensure_schema_migrations
        end
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
        end
        nil
      end

      def self.pending?(files, target)
        i = 0
        while i < files.length
          file = files[i]
          i += 1
          next if file.nil?
          version = file.version
          next if !target.empty? && version > target
          next if version_recorded?(version)
          return true
        end
        false
      end

      def self.version_recorded?(version)
        return false unless ActiveRecord::Base.connection.data_source_exists?("schema_migrations")
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
        # Options (`null:`, `default:`, `limit:`) are not copied onto the
        # rebuilt column. Recording the version after ignoring them would
        # mark a migration applied that did not run.
        raise "unsupported migration statement: #{line}" if parts.length != 3
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
        k = 0
        while k < pks.length
          pk = pks[k]
          raise "change_column cannot preserve composite primary key on #{table}" if pk != 0 && pk != 1
          k += 1
        end
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
      #
      # `PRAGMA index_list` origin `pk` is the rowid primary key. Origin
      # `u` is a UNIQUE autoindex (often named `sqlite_autoindex_*`) and
      # origin `c` is a CREATE INDEX. Skipping every `sqlite_*` name would
      # hide the UNIQUE one. Incoming foreign keys are not on this table's
      # own `foreign_key_list`; with foreign keys on, DROP TABLE cascades
      # through them.
      def self.refuse_unpreserved(table)
        quoted = quote_ident(table)
        # `table_info` does not report CHECK, COLLATE, AUTOINCREMENT, or
        # generated columns. Recording the version after dropping them
        # would mark a migration applied that did not run.
        ddl_stmt = Db.prepare("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = #{ActiveRecord::Base.connection.quote(table)}")
        ddl = ""
        if Db.step?(ddl_stmt)
          ddl = Db.column_text(ddl_stmt, 0).upcase
        end
        Db.finalize(ddl_stmt)
        if keyword?(ddl, "CHECK") || keyword?(ddl, "COLLATE") || keyword?(ddl, "AUTOINCREMENT") || keyword?(ddl, "GENERATED")
          raise "change_column cannot preserve table constraints on #{table}"
        end
        stmt = Db.prepare("PRAGMA index_list(#{quoted})")
        while Db.step?(stmt)
          name = Db.column_text(stmt, 1)
          origin = Db.column_text(stmt, 3)
          next if origin == "pk"
          Db.finalize(stmt)
          raise "change_column cannot preserve index #{name}"
        end
        Db.finalize(stmt)
        listed = Db.prepare("PRAGMA foreign_key_list(#{quoted})")
        if Db.step?(listed)
          Db.finalize(listed)
          raise "change_column cannot preserve foreign keys on #{table}"
        end
        Db.finalize(listed)
        refuse_incoming_foreign_keys(table)
        nil
      end

      # `word` as a SQL keyword. A column named `checked` contains the
      # letters CHECK and is not a constraint.
      def self.keyword?(ddl, word)
        i = 0
        while i < ddl.length
          at = ddl.index(word, i)
          return false if at.nil?
          before = at == 0 ? " " : ddl[at - 1].to_s
          after_at = at + word.length
          after = after_at >= ddl.length ? " " : ddl[after_at].to_s
          ident_before = (before >= "A" && before <= "Z") || (before >= "0" && before <= "9") || before == "_"
          ident_after = (after >= "A" && after <= "Z") || (after >= "0" && after <= "9") || after == "_"
          return true unless ident_before || ident_after
          i = at + word.length
        end
        false
      end

      def self.refuse_incoming_foreign_keys(table)
        master = Db.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        names = []
        while Db.step?(master)
          names << Db.column_text(master, 0)
        end
        Db.finalize(master)
        i = 0
        while i < names.length
          other = names[i].to_s
          i += 1
          next if other == table || other.start_with?("sqlite_")
          listed = Db.prepare("PRAGMA foreign_key_list(#{quote_ident(other)})")
          hit = false
          while Db.step?(listed)
            hit = true if Db.column_text(listed, 2) == table
          end
          Db.finalize(listed)
          raise "change_column cannot preserve foreign keys referencing #{table}" if hit
        end
        nil
      end
    end
  end
end
