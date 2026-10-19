# frozen_string_literal: true

# The `schema.rb` subset `DatabaseTasks.load_schema` runs. Spinel cannot
# `Kernel#load` a file written into a temp directory, so this reads the
# dump as text. Anything outside the subset raises, and that raise
# happens before the version is recorded.
module ActiveRecord
  module Tasks
    module DatabaseTasks
      def self.load_ruby_schema(text)
        lines = text.split("\n")
        version = ""
        # Plan every statement before the first DROP. A `force:` table
        # is replaced as soon as its SQL runs, and each statement is its
        # own autocommit, so a later unsupported line must raise while
        # the database is still untouched.
        statements = []
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
            i = plan_create_table(lines, i - 1, statements)
            next
          end
          next if line == "end" || line.start_with?("ActiveRecord::Schema") || line.include?(".define")
          raise "unsupported schema statement: #{line}"
        end
        k = 0
        while k < statements.length
          execute_sql(statements[k].to_s)
          k += 1
        end
        ensure_schema_migrations
        record_version(version)
        # The dump's version is the latest migration that built it.
        # Older files under `db/migrate` are already applied.
        assume_migrated_upto_version(version)
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

      def self.plan_create_table(lines, index, statements)
        header = lines[index].to_s.strip
        name = table_name_from(header)
        cols = []
        id_kind = "integer"
        at = header.index("id:")
        id_kind = token_at(header[at + 3, header.length].to_s.strip) unless at.nil?
        if id_kind == "false"
          # no id column
        elsif id_kind == "integer" || id_kind == "bigint" || id_kind == "primary_key"
          cols << "id integer PRIMARY KEY"
        else
          raise "unsupported schema statement: #{header}"
        end
        index += 1
        while index < lines.length
          line = lines[index].to_s.strip
          index += 1
          next if line.empty? || line.start_with?("#")
          break if line == "end"
          cols << column_def(line)
        end
        quoted = quote_ident(name)
        statements << "DROP TABLE IF EXISTS #{quoted}" if header.include?("force:")
        statements << "CREATE TABLE #{quoted} (#{cols.join(", ")})"
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
    end
  end
end
