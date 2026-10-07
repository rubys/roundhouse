# Real JDBC failure injection; no Db.bind_* prerequisites are needed, so
# this also exercises the statement-lifecycle commit before binds land.
#   jruby tests/support/jdbc_cleanup_failures.rb
raise "JRuby required" unless RUBY_ENGINE == "jruby"
require_relative "../../runtime/spinel/db_jruby"
require "timeout"

module JdbcCleanupFailures
  class << self
    def check(label, condition)
      raise label unless condition
      @checks += 1
    end

    def failure_case(kind)
      owner = nil
      failed = nil
      failed_rs = nil
      sibling_ps = nil
      sibling_rs = nil
      original_close = nil
      original_raw_close = nil
      request_error = RuntimeError.new("request failed with #{kind}")
      begin
        Db.with_connection do
          owner = Db.current_dbh
          sql = "SELECT id FROM jdbc_cleanup_rows WHERE id >= ? /* #{kind} */"
          if kind == :transient_close
            outer = Db.prepare(sql)
            outer.pstmt.set_long(1, 1)
            Db.step?(outer)
          end
          bad = Db.prepare(sql)
          bad.pstmt.set_long(1, 1)
          Db.step?(bad)
          failed, failed_rs = bad.pstmt, bad.rs
          original_close = failed.method(:close)
          original_raw_close = owner.raw.method(:close)
          case kind
          when :clear
            failed.define_singleton_method(:clear_parameters) { raise "injected clear failure" }
          when :result_set_close
            failed_rs.define_singleton_method(:close) { raise "injected ResultSet close failure" }
          when :cached_close
            failed.define_singleton_method(:clear_parameters) { raise "injected clear failure" }
            failed.define_singleton_method(:close) { raise "injected PreparedStatement close failure" }
            owner.raw.define_singleton_method(:close) { raise "injected connection close failure" }
          when :transient_close
            check("busy hit must be transient", !bad.cached)
            failed.define_singleton_method(:close) { raise "injected transient close failure" }
            owner.raw.define_singleton_method(:close) { raise "injected connection close failure" }
          end
          a = Db.prepare("SELECT id FROM jdbc_cleanup_rows /* #{kind} sibling */")
          b = Db.prepare("SELECT id FROM jdbc_cleanup_rows /* #{kind} sibling */")
          check("first sibling must step", Db.step?(a))
          check("transient sibling must step", Db.step?(b))
          sibling_ps, sibling_rs = b.pstmt, b.rs
          raise request_error
        end
      rescue RuntimeError => e
        check("#{kind}: preserve the request exception object", e.equal?(request_error))
      end
      check("#{kind}: close every transient sibling", sibling_ps.is_closed)
      check("#{kind}: close every sibling ResultSet", sibling_rs.is_closed)
      check("#{kind}: evict the failed cache entry", !owner.stmt_cache.values.include?(failed))
      if kind == :cached_close || kind == :transient_close
        check("#{kind}: failed close retains ownership", owner.open_statements.key?(failed))
        check("#{kind}: quarantine the connection", Db.instance_variable_get(:@quarantined).include?(owner))
        check("#{kind}: remove the connection from the free list", !Db.instance_variable_get(:@free).include?(owner))
        Db.with_connection do
          check("#{kind}: never re-lease quarantine", !Db.current_dbh.equal?(owner))
          st = Db.prepare("SELECT id FROM jdbc_cleanup_rows ORDER BY id")
          check("#{kind}: replacement retains the shared database", Db.step?(st) && Db.column_int(st, 0) == 1)
          Db.finalize(st)
        end
        failed.define_singleton_method(:close) { original_close.call }
        owner.raw.define_singleton_method(:close) { original_raw_close.call }
        Db.release_open_statements(owner)
        check("#{kind}: repaired close releases ownership", owner.open_statements.empty?)
      else
        check("#{kind}: failed statement actually closes", failed.is_closed)
        check("#{kind}: failed ResultSet actually closes", failed_rs.is_closed)
        check("#{kind}: release every owner", owner.open_statements.empty?)
      end
      puts "jdbc cleanup: #{kind} passed"
    end

    def cleanup_error_case
      owner = nil
      bad_ps = nil
      begin
        Db.with_connection do
          owner = Db.current_dbh
          bad = Db.prepare("SELECT 17 AS cleanup_error_without_request")
          bad_ps = bad.pstmt
          bad_ps.define_singleton_method(:clear_parameters) { raise "cleanup error without request" }
          sibling = Db.prepare("SELECT 31 AS cleanup_error_sibling")
          Db.step?(sibling)
        end
        raise "cleanup error was swallowed"
      rescue RuntimeError => e
        check("surface cleanup error after a successful request", e.message == "cleanup error without request")
      end
      check("close the failed cached statement", bad_ps.is_closed)
      check("successful request cleanup drains every sibling", owner.open_statements.empty?)
    end

    def clean_once_case
      Db.with_connection do
        st = Db.prepare("SELECT COALESCE(?, -99) AS clean_once")
        ps = st.pstmt
        clears = 0
        original_clear = ps.method(:clear_parameters)
        ps.define_singleton_method(:clear_parameters) { clears += 1; original_clear.call }
        ps.set_long(1, 73)
        check("clean-on-release seed", Db.step?(st) && Db.column_int(st, 0) == 73)
        Db.finalize(st)
        st = Db.prepare("SELECT COALESCE(?, -99) AS clean_once")
        check("idle reuse adds no duplicate parameter clear", clears == 1)
        check("omitted bind sees NULL on idle reuse", Db.step?(st) && Db.column_int(st, 0) == -99)
        Db.finalize(st)
        check("each release clears once", clears == 2)
      end
    end

    def surrounding_rescue_case
      begin
        raise "unrelated surrounding failure"
      rescue RuntimeError
        begin
          Db.with_connection do
            bad = Db.prepare("SELECT 17 AS cleanup_inside_rescue")
            bad.pstmt.define_singleton_method(:clear_parameters) { raise "cleanup inside outer rescue" }
          end
          raise "outer rescue hid the lease cleanup error"
        rescue RuntimeError => e
          check("successful lease in surrounding rescue surfaces cleanup error", e.message == "cleanup inside outer rescue")
        end
      end
    end

    def checkout_thread
      Thread.new do
        Db.with_connection do
          stmt = Db.prepare("SELECT id FROM jdbc_cleanup_rows ORDER BY id")
          raise "replacement lost the shared database" unless Db.step?(stmt)
          value = Db.column_int(stmt, 0)
          Db.finalize(stmt)
          [Db.current_dbh, value]
        end
      rescue StandardError => e
        e
      end
    end

    def replacement_failure_case(waiting_checkout)
      owner = failed = original_close = waiter = nil
      original_open = Db.method(:open_connection)
      cv = Db.instance_variable_get(:@cv)
      original_wait = cv.method(:wait)
      wait_started = Queue.new
      cv.define_singleton_method(:wait) do |*args|
        wait_started << true
        original_wait.call(*args)
      end
      request_error = RuntimeError.new("request failed before replacement")
      opener_error = Java::JavaSql::SQLException.new("injected replacement failure")
      begin
        Db.with_connection do
          owner = Db.current_dbh
          failed = Db.prepare("SELECT 1 AS failed_replacement").pstmt
          original_close = failed.method(:close)
          failed.define_singleton_method(:clear_parameters) { raise "injected clear failure" }
          failed.define_singleton_method(:close) { raise "injected close failure" }
          Db.define_singleton_method(:open_connection) { raise opener_error }
          if waiting_checkout
            waiter = checkout_thread
            Timeout.timeout(3) { wait_started.pop }
          end
          raise request_error
        end
      rescue RuntimeError => e
        check("replacement failure preserves request error", e.equal?(request_error))
      end
      check("failed replacement retains ownership", owner.open_statements.key?(failed))
      check("failed replacement retains quarantine", Db.instance_variable_get(:@quarantined).include?(owner))
      check("failed replacement leaves no usable connection", Db.instance_variable_get(:@free).empty?)
      waiter ||= checkout_thread
      check("#{waiting_checkout ? 'waiting' : 'new'} checkout slept after replacement failure", waiter.join(3))
      check("checkout surfaces the opener exception object", waiter.value.equal?(opener_error))
      Db.define_singleton_method(:open_connection, original_open)
      waiter = checkout_thread
      check("checkout did not retry the missing connection", waiter.join(3))
      recovered = waiter.value
      raise recovered if recovered.is_a?(Exception)
      check("recovery never leases quarantine", !recovered[0].equal?(owner))
      check("recovery preserves the shared database", recovered[1] == 1)
      check("recovery restores one pool slot", Db.instance_variable_get(:@free).length == 1)
      puts "jdbc cleanup: failed replacement recovers (already waiting: #{waiting_checkout}) passed"
    ensure
      waiter.kill.join if waiter&.alive?
      Db.define_singleton_method(:open_connection, original_open) if original_open
      cv.define_singleton_method(:wait, original_wait) if original_wait
      failed.define_singleton_method(:close, original_close) if original_close
      Db.release_open_statements(owner) if owner
    end

    def original_driver_error_case(operation)
      Db.with_connection do
        actual_driver_error = operation == :step?
        sql = actual_driver_error ? "SELECT abs(-9223372036854775808)" : "SELECT 1 AS metadata_error_identity"
        stmt = Db.prepare(sql)
        ps = stmt.pstmt
        original_execute = ps.method(:execute_query)
        original_close = ps.method(:close)
        sentinel = Java::JavaSql::SQLException.new("injected execute exception")
        original_error = nil
        ps.define_singleton_method(:execute_query) do
          raise sentinel unless actual_driver_error
          original_execute.call
        rescue StandardError => e
          original_error = e
          raise
        end
        ps.define_singleton_method(:close) { raise "cleanup must not replace the driver error" }
        begin
          error = begin
            operation == :column_name ? Db.column_name(stmt, 0) : Db.public_send(operation, stmt)
            nil
          rescue StandardError => e
            e
          end
          expected_class = actual_driver_error ? Java::OrgSqlite::SQLiteException : Java::JavaSql::SQLException
          check("#{operation}: preserve the driver exception class", error.instance_of?(expected_class))
          check("#{operation}: preserve the driver exception object", error.equal?(original_error))
          check("#{operation}: evict the failed statement", !Db.current_dbh.stmt_cache.values.include?(ps))
        ensure
          ps.define_singleton_method(:close, original_close)
          Db.finalize(stmt)
        end
        check("#{operation}: cleanup releases ownership", Db.current_dbh.open_statements.empty?)
      end
      puts "jdbc cleanup: #{operation} preserves the original driver exception passed"
    end

    def idle_cached_shutdown_retry_case
      Db.configure(":memory:", pool_size: 1)
      stmt = Db.prepare("SELECT 1 AS idle_shutdown_retry")
      ps = stmt.pstmt
      owner = Db.current_dbh
      Db.finalize(stmt)
      original_close = ps.method(:close)
      shutdown_error = RuntimeError.new("injected idle cached shutdown close failure")
      ps.define_singleton_method(:close) { raise shutdown_error }
      begin
        Db.close
        raise "idle cached shutdown error was swallowed"
      rescue RuntimeError => e
        check("idle cached shutdown preserves the close exception", e.equal?(shutdown_error))
      ensure
        ps.define_singleton_method(:close, original_close)
      end
      check("idle cached shutdown closes the connection", owner.raw.is_closed)
      check("idle cached shutdown leaves the failed statement open", !ps.is_closed)
      check("idle cached shutdown has no checkout owner", owner.open_statements.empty?)
      retained = Db.instance_variable_get(:@all)
      check("idle cached shutdown retains its connection", retained && retained.include?(owner))
      check("idle cached shutdown retains quarantine", Db.instance_variable_get(:@quarantined).include?(owner))
      Db.close
      check("idle cached shutdown retry closes the statement", ps.is_closed)
      check("idle cached shutdown retry clears all connections", Db.instance_variable_get(:@all).nil?)
      check("idle cached shutdown retry clears quarantine", Db.instance_variable_get(:@quarantined).empty?)
      puts "jdbc cleanup: idle cached shutdown close failure and retry passed"
    end

    def shutdown_retry_case
      owner = Db.current_dbh
      original_close = owner.raw.method(:close)
      owner.raw.define_singleton_method(:close) { raise "injected shutdown close failure" }
      begin
        Db.close
        raise "shutdown error was swallowed"
      rescue RuntimeError => e
        check("surface failed connection shutdown", e.message == "injected shutdown close failure")
      end
      check("failed shutdown retains its connection", Db.instance_variable_get(:@all).include?(owner))
      owner.raw.define_singleton_method(:close) { original_close.call }
      Db.close
      check("retry actually closes the retained connection", owner.raw.is_closed)
      check("successful retry clears all retained connections", Db.instance_variable_get(:@all).nil?)
    end

    def shutdown_release_failure_case
      Db.configure(":memory:", pool_size: 3)
      conns = Db.instance_variable_get(:@all).dup
      statements = []
      first_error = RuntimeError.new("first shutdown release failure")
      conns.each_with_index do |conn, index|
        Fiber[:db_handle] = conn
        idle = Db.prepare("SELECT 37 AS shutdown_idle")
        statements << idle.pstmt
        Db.finalize(idle)
        held = Db.prepare("SELECT 41 AS shutdown_held")
        transient = Db.prepare("SELECT 41 AS shutdown_held")
        statements.push(held.pstmt, transient.pstmt)
        check("shutdown readers step", Db.step?(held) && Db.step?(transient))
        if index < 2
          release_error = index == 0 ? first_error : RuntimeError.new("later shutdown release failure")
          held.pstmt.define_singleton_method(:clear_parameters) { raise release_error }
        end
      end
      Fiber[:db_handle] = nil
      error = begin
        Db.close
        nil
      rescue RuntimeError => e
        e
      end
      check("shutdown closes all statements", statements.all?(&:is_closed))
      check("shutdown closes all connections", conns.all? { |conn| conn.raw.is_closed })
      check("shutdown drains all checkouts", conns.all? { |conn| conn.open_statements.empty? })
      check("shutdown clears free connections", Db.instance_variable_get(:@free).nil?)
      check("shutdown clears all connections", Db.instance_variable_get(:@all).nil?)
      check("shutdown clears quarantine", Db.instance_variable_get(:@quarantined).empty?)
      check("shutdown preserves first release error", error.equal?(first_error))
      Db.close
      puts "jdbc cleanup: shutdown release failure drains all connections passed"
    end

    def run
      @checks = 0
      Db.configure("file:jdbc_cleanup_failures?mode=memory&cache=shared", pool_size: 1)
      Db.exec("CREATE TABLE jdbc_cleanup_rows (id INTEGER)")
      Db.exec("INSERT INTO jdbc_cleanup_rows VALUES (1), (2)")
      [:clear, :result_set_close, :cached_close, :transient_close].each { |kind| failure_case(kind) }
      cleanup_error_case
      surrounding_rescue_case
      clean_once_case
      replacement_failure_case(false)
      replacement_failure_case(true)
      original_driver_error_case(:step?)
      original_driver_error_case(:column_count)
      original_driver_error_case(:column_name)
      Db.exec("DROP TABLE jdbc_cleanup_rows")
      shutdown_retry_case
      idle_cached_shutdown_retry_case
      shutdown_release_failure_case
      puts "jdbc cleanup: #{@checks} assertions passed"
    ensure
      Db.close
    end
  end
end

JdbcCleanupFailures.run
