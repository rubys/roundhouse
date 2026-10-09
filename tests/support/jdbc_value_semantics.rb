# Run on a real JVM: jruby tests/support/jdbc_value_semantics.rb
# Requires JRuby 10+ and jdbc-sqlite3. ROUNDHOUSE_SOURCE is only for an
# external probe; once copied under tests/support the repository is inferred.
raise "JRuby required" unless RUBY_ENGINE == "jruby"

require "json"
source = ENV.fetch("ROUNDHOUSE_SOURCE") { File.expand_path("../..", __dir__) }
require File.join(source, "runtime/spinel/db_jruby")
require File.join(source, "runtime/spinel/scaffold/ruby_overlay/runtime/active_support_time_parsing")

module JdbcValueSemantics
  class << self
    attr_accessor :checks

    def equal(label, expected, actual)
      raise "#{label}: expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
      self.checks += 1
    end

    def truth(label, actual)
      equal(label, true, !!actual)
    end

    def rows(sql, binds = [], uncached: false)
      stmt = uncached ? Db.prepare_uncached(sql) : Db.prepare(sql)
      binds.each_with_index { |(method, value), index| Db.public_send(method, stmt, index + 1, value) }
      result = []
      while Db.step?(stmt)
        result << Array.new(Db.column_count(stmt)) { |index| Db.column_text_opt(stmt, index) }
      end
      result
    ensure
      Db.finalize(stmt) if stmt
    end

    def string_cases
      Db.exec("CREATE TABLE writer_values (id INTEGER PRIMARY KEY, value BLOB)")
      cases = [
        ["empty UTF-8", "", "text"],
        ["quoted ASCII UTF-8", "a'b\\c", "text"],
        ["multibyte UTF-8", "雪 café 😀", "text"],
        ["NUL UTF-8", "a\0雪", "blob"],
        ["empty BINARY", "".b, "text"],
        ["ASCII BINARY", "ascii ' \\".b, "text"],
        ["NUL BINARY", "a\0b".b, "blob"],
        ["non-ASCII BINARY", [0xff, 0x80, 0x61].pack("C*"), "blob"],
        ["UTF-8 bytes tagged BINARY", "雪 😀".b, "blob"],
        ["invalid UTF-8 with NUL", [0xff, 0x00, 0x80, 0x61].pack("C*").force_encoding(Encoding::UTF_8), "blob"]
      ]
      cases.each_with_index do |(label, value, kind), index|
        id = index + 1
        hex = value.unpack1("H*").upcase
        Db.exec("INSERT INTO writer_values VALUES (#{id}, #{Db.escape_string(value)})")
        equal("#{label}: writer storage and bytes", [[kind, hex]],
          rows("SELECT typeof(value), hex(value) FROM writer_values WHERE id = #{id}"))
        equal("#{label}: binder storage and bytes", [[kind, hex]],
          rows("SELECT typeof(?), hex(?)", [[:bind_text, value], [:bind_text, value]]))
        equal("#{label}: bound equality finds inline writer", [[id.to_s]],
          rows("SELECT id FROM writer_values WHERE id = #{id} AND value = ?", [[:bind_text, value]]))

        # Drop every reference to the input bytes before stepping. This
        # checks the actual JDBC value after mutation and collection.
        stmt = Db.prepare("SELECT typeof(?), hex(?)")
        input = value.dup
        Db.bind_text(stmt, 1, input)
        Db.bind_text(stmt, 2, input)
        input.replace("replaced")
        input = nil
        GC.start
        truth("#{label}: copied bind returns a row", Db.step?(stmt))
        equal("#{label}: copied bind type", kind, Db.column_text(stmt, 0))
        equal("#{label}: copied bind bytes", hex, Db.column_text(stmt, 1))
        Db.finalize(stmt)
        puts JSON.generate(case: label, storage_class: kind, byte_hex: hex, writer_binder_equal: true)
      end
    end

    def optional_cases
      [:bind_int_opt, :bind_text_opt, :bind_bool_opt].each do |method|
        truth("optional primitive #{method} exists", Db.respond_to?(method))
      end
      Db.exec("CREATE TABLE nullable_rows (id INTEGER PRIMARY KEY, number INTEGER, name TEXT, flag BOOLEAN)")
      cases = [[1, nil, nil, nil], [2, 0, "", false], [3, -17, "雪", true], [4, 2**40 + 123, "binary\0name".b, nil]]
      cases.each do |id, number, name, flag|
        Db.exec("INSERT INTO nullable_rows VALUES (#{id}, #{Db.escape_int_opt(number)}, #{Db.escape_string_opt(name)}, #{Db.escape_bool_opt(flag)})")
      end
      cached = {}
      [1, 3, 2, 4, 1, 4, 2, 3, 1].each do |id|
        _, number, name, flag = cases.fetch(id - 1)
        binds = []
        predicates = [["number", :bind_int_opt, number], ["name", :bind_text_opt, name],
                      ["flag", :bind_bool_opt, flag]].map do |column, method, value|
          if value.nil?
            "#{column} IS NULL"
          else
            binds << [method, value]
            "#{column} = ?"
          end
        end
        sql = "SELECT id FROM nullable_rows WHERE " + predicates.join(" AND ")
        stmt = Db.prepare(sql)
        truth("optional prepare is lazy", !stmt.executed)
        truth("optional values reuse their JDBC shape", cached[sql].equal?(stmt.pstmt)) if cached[sql]
        cached[sql] = stmt.pstmt
        equal("nil predicates remove slots", binds.length, sql.count("?"))
        binds.each_with_index { |(method, value), index| Db.public_send(method, stmt, index + 1, value) }
        truth("optional tuple #{id} matches", Db.step?(stmt))
        equal("optional tuple #{id} lockstep", id, Db.column_int(stmt, 0))
        truth("optional tuple #{id} exact cardinality", !Db.step?(stmt))
        Db.finalize(stmt)
      end
      equal("optional tuples have three null patterns", 3, cached.length)
      [[:bind_int_opt, nil, "null"], [:bind_int_opt, 0, "integer"],
       [:bind_text_opt, nil, "null"], [:bind_text_opt, "", "text"],
       [:bind_bool_opt, nil, "null"], [:bind_bool_opt, false, "integer"],
       [:bind_bool_opt, true, "integer"]].each do |method, value, kind|
        equal("#{method} #{value.inspect} storage", [[kind]], rows("SELECT typeof(?)", [[method, value]]))
      end
      [nil, false, true].each do |value|
        expected = value.nil? ? ["null", "-7"] : ["integer", value ? "1" : "0"]
        equal("nullable primitive bool #{value.inspect}", [expected],
          rows("SELECT typeof(?), COALESCE(?, -7)", [[:bind_bool, value], [:bind_bool, value]]))
      end
      puts "optional int/text/bool: NULL, zero, empty text, false, true, binary and 64-bit values pass"
    end

    # Compare with SQL written and read on this JVM. Float and temporal
    # values reach bind_text only after the lowerer's writer serialization.
    def scalar_cases
      stamp = Time.at(1_700_000_000, 123456, :microsecond)
      groups = [
        ["integer", "INTEGER", :bind_int_opt,
         [nil, 0, -1, 2**31 - 1, 2**31, -(2**31), -(2**31) - 1, 2**40 + 123, 2**63 - 1, -(2**63)]],
        ["float", "REAL", :bind_text_opt,
         [nil, 0.0, -0.0, 0.1 + 0.2, 0.3, 1e-7, -1e20, Float::MIN, Float::MAX, 5e-324]],
        ["date", "TEXT", :bind_text_opt,
         [nil, Date.new(2024, 2, 29), Date.new(1970, 1, 1), Date.new(1, 1, 1)]],
        ["time", "TEXT", :bind_text_opt,
         [nil, stamp.utc, stamp.getlocal("-04:00"), stamp.getlocal("+05:45"),
          stamp.getlocal("+14:00"), Time.at(-1, 999999, :microsecond).utc]],
        ["boolean", "BOOLEAN", :bind_bool_opt, [nil, false, true]]
      ]
      groups.each do |kind, affinity, method, values|
        table = "scalar_#{kind}"
        Db.exec("CREATE TABLE #{table} (id INTEGER PRIMARY KEY, value #{affinity})")
        serialized = values.map do |value|
          case kind
          when "float" then value.nil? ? nil : value.to_s
          when "date" then ActiveSupport.format_db_date(value)
          when "time" then ActiveSupport.format_db_time(value)
          else value
          end
        end
        literals = values.each_with_index.map do |value, i|
          case kind
          when "integer" then Db.escape_int_opt(value)
          when "float" then Db.escape_float_opt(value)
          when "boolean" then Db.escape_bool_opt(value)
          else Db.escape_string_opt(serialized[i])
          end
        end
        literals.each_with_index { |literal, i| Db.exec("INSERT INTO #{table} VALUES (#{i}, #{literal})") }
        values.each_with_index do |value, i|
          predicate = value.nil? ? "value IS NULL" : "value = #{literals[i]}"
          bound_predicate = value.nil? ? "value IS NULL" : "value = ?"
          binds = value.nil? ? [] : [[method, serialized[i]]]
          query = "SELECT id, typeof(value), quote(value), hex(value) FROM #{table} WHERE "
          equal("#{kind} #{value.inspect}: same-JRuby inline writer/read parity",
            rows(query + predicate + " ORDER BY id"),
            rows(query + bound_predicate + " ORDER BY id", binds))
          equal("#{kind} nil consumes no slot", value.nil? ? 0 : 1, binds.length)
        end
        puts "#{kind}: #{values.length} inline/bound comparisons pass on #{RUBY_DESCRIPTION}"
      end
    end

    def transient_cases
      Db.exec("CREATE TABLE in_rows (id INTEGER PRIMARY KEY)")
      (1..6).each { |id| Db.exec("INSERT INTO in_rows VALUES (#{id})") }
      conn = Db.current_dbh
      sql = "SELECT id FROM in_rows WHERE id IN (1, 2, 3) ORDER BY id"
      # Even if the same SQL already has a cached statement, the IN path
      # must create separate live handles and leave the cached one alone.
      cached_stmt = Db.prepare(sql)
      cached = cached_stmt.pstmt
      Db.finalize(cached_stmt)
      cache_size = conn.stmt_cache.size
      first = Db.prepare_uncached(sql)
      second = Db.prepare_uncached(sql)
      first_ps, second_ps = first.pstmt, second.pstmt
      truth("overlapping IN statements are distinct", !first_ps.equal?(second_ps))
      truth("uncached IN bypasses existing cache", !first_ps.equal?(cached) && !second_ps.equal?(cached))
      truth("first IN step", Db.step?(first))
      equal("first IN starts at one", 1, Db.column_int(first, 0))
      truth("second IN step", Db.step?(second))
      equal("second IN starts at one", 1, Db.column_int(second, 0))
      truth("first IN retains its cursor", Db.step?(first))
      equal("first IN advances independently", 2, Db.column_int(first, 0))
      Db.finalize(first)
      truth("first IN closes on finalize", first_ps.is_closed)
      truth("second IN stays open", !second_ps.is_closed)
      truth("second IN retains its cursor", Db.step?(second))
      equal("second IN advances independently", 2, Db.column_int(second, 0))
      Db.finalize(second)
      truth("second IN closes on finalize", second_ps.is_closed)
      truth("cached statement stays open", !cached.is_closed)
      equal("IN leaves statement cache unchanged", cache_size, conn.stmt_cache.size)
      [[1], [2, 5], [6, 3, 1], []].each do |ids|
        expected = ids.sort.map { |id| [id.to_s] }
        equal("IN cardinality #{ids.length}", expected,
          rows("SELECT id FROM in_rows WHERE id IN (#{Db.escape_int_list(ids)}) ORDER BY id", uncached: true))
      end
      equal("varying IN leaves statement cache unchanged", cache_size, conn.stmt_cache.size)

      Db.query_cache_begin
      begin
        first = Db.prepare_uncached(sql)
        original = first.pstmt
        truth("partial IN row", Db.step?(first))
        Db.finalize(first)
        truth("partial IN original closed", original.is_closed)
        second = Db.prepare_uncached(sql)
        truth("IN result replay has no JDBC statement", second.pstmt.nil?)
        truth("IN replay prefix", Db.step?(second))
        equal("IN replay prefix value", 1, Db.column_int(second, 0))
        truth("IN replay promotes", Db.step?(second))
        promoted = second.pstmt
        truth("promoted IN is a real statement", !promoted.nil?)
        equal("IN replay resumes after prefix", 2, Db.column_int(second, 0))
        truth("promoted IN remains transient", !second.cached)
        Db.finalize(second)
        truth("promoted IN closes", promoted.is_closed)
        equal("replay promotion leaves cache unchanged", cache_size, conn.stmt_cache.size)
      ensure
        Db.query_cache_end
      end
      equal("finalized IN leaves no live handles", 0, conn.open_statements.size)
      puts "IN: varying arity, overlap, existing-cache bypass, replay promotion and finalize cleanup pass"
    end

    def binding_failure_cases
      [[:bind_int, :set_long, 7], [:bind_int_opt, :set_null, nil],
       [:bind_text, :set_string, "snow 雪"], [:bind_text, :set_bytes, "a\0b"],
       [:bind_text_opt, :set_string, "text"], [:bind_text_opt, :set_null, nil],
       [:bind_bool, :set_int, false], [:bind_bool, :set_null, nil],
       [:bind_bool_opt, :set_int, true]].each do |method, setter, value|
        sql = "SELECT ? /* #{method} #{setter} failure */"
        stmt = Db.prepare(sql)
        ps = stmt.pstmt
        close = ps.method(:close)
        error = Java::JavaSql::SQLException.new("injected #{setter} failure")
        ps.define_singleton_method(setter) { |*args| raise error }
        ps.define_singleton_method(:close) { raise "injected disposal failure" }
        begin
          Db.public_send(method, stmt, 1, value)
          raise "#{method} swallowed the setter failure"
        rescue StandardError => actual
          truth("#{method}/#{setter}: preserve driver exception", actual.equal?(error))
          truth("#{method}/#{setter}: evict failed checkout", !Db.current_dbh.stmt_cache.key?(sql))
          truth("#{method}/#{setter}: failed disposal stays owned", Db.current_dbh.open_statements.key?(ps))
        ensure
          ps.define_singleton_method(:close, close)
          Db.finalize(stmt)
        end
        truth("#{method}/#{setter}: retry closes statement", ps.is_closed)
        truth("#{method}/#{setter}: retry releases ownership", !Db.current_dbh.open_statements.key?(ps))
      end
      [:bind_int, :bind_int_opt, :bind_text, :bind_text_opt, :bind_bool, :bind_bool_opt].each do |method|
        stmt = Db.prepare("SELECT ? /* #{method} late bind */")
        ps = stmt.pstmt
        truth("#{method}: initial query steps", Db.step?(stmt))
        rs = stmt.rs
        begin
          Db.public_send(method, stmt, 1, nil)
          raise "#{method} accepted a late bind"
        rescue RuntimeError => error
          equal("#{method}: late bind rejected", "statement is not bindable", error.message)
        end
        truth("#{method}: late bind closes ResultSet", rs.is_closed)
        truth("#{method}: late bind closes statement", ps.is_closed)
        begin
          Db.public_send(method, stmt, 1, nil)
          raise "#{method} accepted a released handle"
        rescue RuntimeError => error
          equal("#{method}: released handle rejected", "statement is not bindable", error.message)
        end
      end
      equal("binding failures leave no owned statements", 0, Db.current_dbh.open_statements.size)
    end

    def cleanup_cases
      owner = nil
      abandoned = []
      begin
        Db.with_connection do
          owner = Db.current_dbh
          2.times do
            stmt = Db.prepare_uncached("SELECT id FROM in_rows WHERE id IN (1, 2) ORDER BY id")
            Db.step?(stmt)
            abandoned << [stmt.pstmt, stmt.rs]
          end
          raise "expected unwind"
        end
      rescue RuntimeError => error
        raise unless error.message == "expected unwind"
      end
      abandoned.each do |ps, rs|
        truth("lease unwind closes IN PreparedStatement", ps.is_closed)
        truth("lease unwind closes IN ResultSet", rs.is_closed)
      end
      equal("lease unwind clears ownership", 0, owner.open_statements.size)
      outside = Db.prepare_uncached("SELECT id FROM in_rows WHERE id IN (2, 4)")
      Db.step?(outside)
      ps, rs, raw = outside.pstmt, outside.rs, Db.current_dbh.raw
      Db.close
      truth("Db.close closes out-of-lease IN statement", ps.is_closed)
      truth("Db.close closes out-of-lease result set", rs.is_closed)
      truth("Db.close closes JDBC connection", raw.is_closed)
      puts "IN cleanup: exception unwind and pool close pass with actual JDBC isClosed assertions"
    end

    def run
      self.checks = 0
      Db.configure(":memory:", pool_size: 1)
      puts "#{RUBY_DESCRIPTION}; SQLite #{rows('SELECT sqlite_version()').first.first}"
      Db.with_connection do
        string_cases
        optional_cases
        scalar_cases
        binding_failure_cases
        transient_cases
      end
      cleanup_cases
      puts JSON.generate(result: "PASS", assertions: checks)
    ensure
      Db.close
    end
  end
end

JdbcValueSemantics.run
