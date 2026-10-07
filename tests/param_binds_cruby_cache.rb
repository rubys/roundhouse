# Snapshot cleanup errors must not bypass reader cleanup or replace the
# request's exception. The failed COMMIT still gets a ROLLBACK.
[false, true].each do |request_failed|
  conn = transient = original_execute = nil
  snapshot_error = RuntimeError.new("injected snapshot COMMIT failure")
  request_error = RuntimeError.new("request failed inside snapshot")
  begin
    Db.with_connection do
      conn = Db.current_dbh
      original_execute = conn.method(:execute)
      conn.define_singleton_method(:execute) do |sql, *args|
        raise snapshot_error if sql == "COMMIT"
        original_execute.call(sql, *args)
      end
      Db.read_snapshot_begin
      outer = Db.prepare("SELECT 1 AS snapshot_cleanup_ownership")
      inner = Db.prepare("SELECT 1 AS snapshot_cleanup_ownership")
      transient = inner[:stmt]
      Db.step?(outer)
      Db.step?(inner)
      raise request_error if request_failed
    end
    raise "snapshot cleanup failure was swallowed"
  rescue RuntimeError => e
    expected = request_failed ? request_error : snapshot_error
    raise "snapshot cleanup replaced the exception" unless e.equal?(expected)
  ensure
    conn.define_singleton_method(:execute, original_execute) if original_execute
  end
  raise "snapshot cleanup retained readers" unless Db.open_statements(conn).empty?
  raise "snapshot cleanup retained its sibling" unless transient.closed?
  raise "snapshot cleanup retained its transaction" if conn.transaction_active?
  raise "snapshot cleanup retained its lease" if Db.in_lease?
  raise "snapshot cleanup lost its usable connection" unless Db.instance_variable_get(:@pool).free.include?(conn)
end
puts "runtime: snapshot failure drains readers and preserves exception identity passed"

# A failed rollback releases the permit, drains readers and quarantines
# the connection while its transaction is still active.
conn = original_execute = nil
request_error = RuntimeError.new("request failed before rollback")
begin
  Db.with_connection do
    conn = Db.current_dbh
    original_execute = conn.method(:execute)
    conn.define_singleton_method(:execute) do |sql, *args|
      raise "injected ROLLBACK failure" if sql == "ROLLBACK"
      original_execute.call(sql, *args)
    end
    Db.exec("BEGIN")
    Db.prepare("SELECT 1 AS rollback_cleanup_ownership")
    raise request_error
  end
rescue RuntimeError => e
  raise "rollback cleanup replaced the request exception" unless e.equal?(request_error)
ensure
  conn.define_singleton_method(:execute, original_execute) if original_execute
end
raise "rollback cleanup retained readers" unless Db.open_statements(conn).empty?
raise "rollback cleanup retained the permit" if Db.permit_owned?
raise "rollback failure returned its connection" if Db.instance_variable_get(:@pool).free.include?(conn)
raise "rollback failure lost quarantine" unless Db.instance_variable_get(:@quarantined).include?(conn)
puts "runtime: rollback failure drains readers and quarantines the connection passed"

# Gem-level lifecycle observations complement the cross-runtime row checks.
Db.with_connection do
  outer = Db.prepare("SELECT ? AS transient_ownership")
  cached = outer[:stmt]
  inner = Db.prepare("SELECT ? AS transient_ownership")
  transient = inner[:stmt]
  raise "busy hit aliased the cached statement" if cached.equal?(transient)
  Db.finalize(inner)
  raise "transient was not really closed" if !transient.closed?
  Db.finalize(outer)
  raise "cached statement was closed" if cached.closed?
  reused = Db.prepare("SELECT ? AS transient_ownership")
  raise "idle hit stopped caching" if !reused[:stmt].equal?(cached)
  Db.finalize(reused)
end

# A failed reset must not skip abandoned siblings or return a poisoned
# cache entry. Preserve the request exception object, not just its text.
Db.exec("CREATE TABLE cleanup_failure_rows (id INTEGER)")
Db.exec("INSERT INTO cleanup_failure_rows VALUES (1), (2)")
failed_conn = nil
failed_cached = nil
sibling_transient = nil
request_error = RuntimeError.new("request failed before cleanup")
begin
  Db.with_connection do
    failed_conn = Db.current_dbh
    bad = Db.prepare("SELECT 0 AS cleanup_reset_failure")
    failed_cached = bad[:stmt]
    failed_cached.define_singleton_method(:reset!) { raise "injected reset failure" }
    a = Db.prepare("SELECT id FROM cleanup_failure_rows")
    b = Db.prepare("SELECT id FROM cleanup_failure_rows")
    sibling_transient = b[:stmt]
    raise "missing cleanup sibling" if !Db.step?(a) || !Db.step?(b)
    raise request_error
  end
rescue RuntimeError => e
  raise "cleanup replaced the request exception" unless e.equal?(request_error)
end
raise "failed reset left owned cursors" unless Db.open_statements(failed_conn).empty?
raise "failed cached statement was not closed" unless failed_cached.closed?
raise "failed cached statement was not evicted" if failed_conn.instance_variable_get(:@rh_stmt_cache).values.include?(failed_cached)
raise "cleanup skipped a transient sibling" unless sibling_transient.closed?
Db.exec("DROP TABLE cleanup_failure_rows")

# Clear failure has the same discard contract; a successful request must
# still surface its cleanup error after every sibling has been released.
failed_cached = nil
begin
  Db.with_connection do
    bad = Db.prepare("SELECT 0 AS cleanup_clear_failure")
    failed_cached = bad[:stmt]
    failed_cached.define_singleton_method(:clear_bindings!) { raise "injected clear failure" }
    a = Db.prepare("SELECT 1 AS cleanup_clear_sibling")
    b = Db.prepare("SELECT 1 AS cleanup_clear_sibling")
    sibling_transient = b[:stmt]
    Db.step?(a)
    Db.step?(b)
  end
  raise "cleanup failure was swallowed"
rescue RuntimeError => e
  raise e if e.message != "injected clear failure"
end
raise "clear failure kept the cached statement" unless failed_cached.closed?
raise "clear failure skipped a transient sibling" unless sibling_transient.closed?

# $! may belong to an enclosing rescue, rather than this request. A
# successful lease inside a rescue still surfaces its own cleanup error.
begin
  raise "unrelated surrounding failure"
rescue RuntimeError
  begin
    Db.with_connection do
      bad = Db.prepare("SELECT 0 AS cleanup_inside_rescue")
      bad[:stmt].define_singleton_method(:reset!) { raise "cleanup inside outer rescue" }
    end
    raise "outer rescue hid the lease cleanup error"
  rescue RuntimeError => e
    raise e if e.message != "cleanup inside outer rescue"
  end
end

# If close itself keeps failing, ownership must remain visible and that
# connection must never be returned to the pool. Replacement also keeps a
# one-connection pool usable; the gate uses a shared in-memory database.
quarantined = nil
stuck = nil
real_close = nil
request_error = RuntimeError.new("request failed with an unclosable cursor")
begin
  Db.with_connection do
    quarantined = Db.current_dbh
    bad = Db.prepare("SELECT 0 AS cleanup_close_failure")
    stuck = bad[:stmt]
    real_close = stuck.method(:close)
    stuck.define_singleton_method(:reset!) { raise "injected reset failure" }
    stuck.define_singleton_method(:close) { raise "injected close failure" }
    a = Db.prepare("SELECT 1 AS cleanup_close_sibling")
    b = Db.prepare("SELECT 1 AS cleanup_close_sibling")
    sibling_transient = b[:stmt]
    Db.step?(a)
    Db.step?(b)
    raise request_error
  end
rescue RuntimeError => e
  raise "unclosable cursor replaced request error" unless e.equal?(request_error)
end
raise "failed close forgot its ownership" unless Db.open_statements(quarantined).key?(stuck)
raise "failed close skipped a transient sibling" unless sibling_transient.closed?
raise "failed close returned its connection to the pool" if Db.instance_variable_get(:@pool).free.include?(quarantined)
raise "failed close lost the quarantined connection" unless Db.instance_variable_get(:@quarantined).include?(quarantined)
Db.with_connection do
  raise "quarantined connection was leased again" if Db.current_dbh.equal?(quarantined)
  stmt = Db.prepare("SELECT 71 AS replacement_is_usable")
  raise "replacement connection is unusable" unless Db.step?(stmt) && Db.column_int(stmt, 0) == 71
  Db.finalize(stmt)
end
stuck.define_singleton_method(:close) { real_close.call }
Db.release_open_statements(quarantined)
puts "runtime: cleanup failures drain siblings, evict, quarantine and preserve request errors passed"

# An idle hit is already clean. Only release resets and clears bindings.
Db.with_connection do
  stmt = Db.prepare("SELECT ? AS cleanup_once")
  raw = stmt[:stmt]
  resets = 0
  clears = 0
  real_reset = raw.method(:reset!)
  real_clear = raw.method(:clear_bindings!)
  raw.define_singleton_method(:reset!) { resets += 1; real_reset.call }
  raw.define_singleton_method(:clear_bindings!) { clears += 1; real_clear.call }
  Db.bind_int(stmt, 1, 19)
  Db.step?(stmt)
  Db.finalize(stmt)
  reused = Db.prepare("SELECT ? AS cleanup_once")
  raise "idle checkout duplicated reset or clear" unless resets == 1 && clears == 1
  raise "clean idle checkout retained a binding" unless Db.step?(reused) && Db.column_int(reused, 0) == 0
  Db.finalize(reused)
  raise "release did not clean exactly once" unless resets == 2 && clears == 2
end
puts "runtime: cached release cleans exactly once passed"

# A cache eviction happens after the fresh prepare but before its normal
# ownership registration. A failed eviction close must retain both native
# statements, close the fresh one, and quarantine an unclosable evictee.
eviction_conn = nil
evicted = nil
fresh = nil
real_evicted_close = nil
real_prepare = nil
old_cap = Db::STMT_CACHE_CAP
eviction_error = RuntimeError.new("injected eviction close failure")
begin
  Db.with_connection do
    eviction_conn = Db.current_dbh
    seed = Db.prepare("SELECT 11 AS eviction_ownership_seed")
    Db.finalize(seed)
    cache = eviction_conn.instance_variable_get(:@rh_stmt_cache)
    evicted = cache.values.first
    real_evicted_close = evicted.method(:close)
    evicted.define_singleton_method(:close) { raise eviction_error }
    real_prepare = eviction_conn.method(:prepare)
    eviction_conn.define_singleton_method(:prepare) do |sql|
      fresh = real_prepare.call(sql)
      fresh
    end
    Db.send(:remove_const, :STMT_CACHE_CAP)
    Db.const_set(:STMT_CACHE_CAP, cache.size)
    Db.prepare("SELECT 29 AS eviction_ownership_fresh")
    raise "eviction error was swallowed"
  end
rescue RuntimeError => e
  raise "eviction cleanup replaced the request exception" unless e.equal?(eviction_error)
ensure
  Db.send(:remove_const, :STMT_CACHE_CAP)
  Db.const_set(:STMT_CACHE_CAP, old_cap)
end
raise "eviction cleanup leaked the fresh prepared statement" unless fresh.closed?
raise "eviction failed close forgot its ownership" unless Db.open_statements(eviction_conn).key?(evicted)
raise "eviction failed close returned its connection" if Db.instance_variable_get(:@pool).free.include?(eviction_conn)
raise "eviction failed close lost quarantine" unless Db.instance_variable_get(:@quarantined).include?(eviction_conn)
evicted.define_singleton_method(:close) { real_evicted_close.call }
Db.release_open_statements(eviction_conn)
raise "eviction repaired close retained ownership" unless Db.open_statements(eviction_conn).empty?
puts "runtime: eviction close failure owns both statements and quarantines safely passed"

require "timeout"

module CrubyCleanupRegressions
  @checks = 0

  def self.check(label, condition)
    raise label unless condition
    @checks += 1
  end

  def self.checkout_thread
    Thread.new do
      Db.with_connection do
        stmt = Db.prepare("SELECT id FROM replacement_failure_rows")
        raise "replacement lost the shared database" unless Db.step?(stmt)
        value = Db.column_int(stmt, 0)
        Db.finalize(stmt)
        [Db.current_dbh, value]
      end
    rescue StandardError => e
      e
    end
  end

  def self.replacement_failure_case(waiting_checkout)
    Db.close
    Db.configure("file:replacement_failure?mode=memory&cache=shared", pool_size: 1)
    Db.exec("CREATE TABLE replacement_failure_rows (id INTEGER)")
    Db.exec("INSERT INTO replacement_failure_rows VALUES (83)")
    owner = stuck = original_close = waiter = nil
    original_open = Db.method(:open_connection)
    cv = Db.instance_variable_get(:@cv)
    original_wait = cv.method(:wait)
    wait_started = Queue.new
    cv.define_singleton_method(:wait) do |*args|
      wait_started << true
      original_wait.call(*args)
    end
    request_error = RuntimeError.new("request failed before replacement")
    opener_error = SQLite3::CantOpenException.new("injected replacement failure")
    begin
      Db.with_connection do
        owner = Db.current_dbh
        stuck = Db.prepare("SELECT 1 AS failed_replacement")[:stmt]
        original_close = stuck.method(:close)
        stuck.define_singleton_method(:reset!) { raise "injected reset failure" }
        stuck.define_singleton_method(:close) { raise "injected close failure" }
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
    check("failed replacement retains ownership", Db.open_statements(owner).key?(stuck))
    check("failed replacement retains quarantine", Db.instance_variable_get(:@quarantined).include?(owner))
    check("failed replacement leaves no usable connection", Db.instance_variable_get(:@pool).available_count == 0)
    waiter ||= checkout_thread
    check("#{waiting_checkout ? 'waiting' : 'new'} checkout slept after replacement failure", waiter.join(3))
    check("checkout surfaces the opener exception object", waiter.value.equal?(opener_error))
    Db.define_singleton_method(:open_connection, original_open)
    waiter = checkout_thread
    check("checkout did not retry the missing connection", waiter.join(3))
    recovered = waiter.value
    raise recovered if recovered.is_a?(Exception)
    check("recovery never leases quarantine", !recovered[0].equal?(owner))
    check("recovery preserves the shared database", recovered[1] == 83)
    check("recovery restores one pool slot", Db.instance_variable_get(:@pool).available_count == 1)
    puts "runtime: failed replacement recovers (already waiting: #{waiting_checkout}) passed"
  ensure
    waiter.kill.join if waiter&.alive?
    Db.define_singleton_method(:open_connection, original_open) if original_open
    cv.define_singleton_method(:wait, original_wait) if original_wait
    stuck.define_singleton_method(:close, original_close) if original_close
    Db.release_open_statements(owner) if owner
  end

  def self.original_driver_error_case(operation)
    Db.with_connection do
      sql = operation == :step ? "SELECT abs(-9223372036854775808)" : "SELECT ? AS bind_error_identity"
      stmt = Db.prepare(sql)
      raw = stmt[:stmt]
      driver_name = operation == :step ? :step : :bind_param
      driver_call = raw.method(driver_name)
      original_error = nil
      raw.define_singleton_method(driver_name) do |*args|
        driver_call.call(*args)
      rescue SQLite3::Exception => e
        original_error = e
        raise
      end
      raw.define_singleton_method(:reset!) { raise "cleanup must not replace the driver error" }
      error = begin
        operation == :step ? Db.step?(stmt) : Db.bind_int(stmt, 2, 73)
        nil
      rescue StandardError => e
        e
      end
      expected_class = operation == :step ? SQLite3::SQLException : SQLite3::RangeException
      check("#{operation}: preserve the driver exception class", error.instance_of?(expected_class))
      check("#{operation}: preserve the driver exception object", error.equal?(original_error))
      check("#{operation}: cleanup closes the failed statement", raw.closed?)
      check("#{operation}: cleanup releases ownership", stmt[:stmt].nil?)
    end
    puts "runtime: #{operation} preserves the original driver exception passed"
  end
end

CrubyCleanupRegressions.replacement_failure_case(false)
CrubyCleanupRegressions.replacement_failure_case(true)
CrubyCleanupRegressions.original_driver_error_case(:step)
CrubyCleanupRegressions.original_driver_error_case(:bind)
puts "runtime: #{CrubyCleanupRegressions.instance_variable_get(:@checks)} replacement and exception assertions passed"

# The unleased boot/script path also owns its unfinished statements at close.
outer = Db.prepare("SELECT ? AS shutdown_ownership")
inner = Db.prepare("SELECT ? AS shutdown_ownership")
cached, transient = outer[:stmt], inner[:stmt]
closing_conn = Db.current_dbh
real_connection_close = closing_conn.method(:close)
closing_conn.define_singleton_method(:close) { raise "injected shutdown close failure" }
begin
  Db.close
  raise "shutdown error was swallowed"
rescue RuntimeError => e
  raise e if e.message != "injected shutdown close failure"
end
raise "shutdown leaked the cached statement" if !cached.closed?
raise "shutdown leaked the transient statement" if !transient.closed?
raise "shutdown forgot a failed connection close" unless Db.instance_variable_get(:@quarantined).include?(closing_conn)
closing_conn.define_singleton_method(:close) { real_connection_close.call }
Db.close
raise "shutdown retry did not close the connection" unless closing_conn.closed?
puts "runtime: gem transient release, shutdown and failed-close retry passed"

# An idle cached statement has no checkout owner. Even if its connection
# closes successfully, a failed statement close must remain retryable.
Db.configure(":memory:", pool_size: 1)
idle = Db.prepare("SELECT 1 AS idle_shutdown_retry")
idle_cached = idle[:stmt]
idle_conn = Db.current_dbh
Db.finalize(idle)
real_idle_close = idle_cached.method(:close)
shutdown_error = RuntimeError.new("injected idle cached shutdown close failure")
idle_cached.define_singleton_method(:close) { raise shutdown_error }
begin
  Db.close
  raise "idle cached shutdown error was swallowed"
rescue RuntimeError => e
  raise "idle cached shutdown replaced the close exception" unless e.equal?(shutdown_error)
end
raise "idle cached shutdown did not close the connection" unless idle_conn.closed?
raise "idle cached shutdown unexpectedly closed the statement" if idle_cached.closed?
raise "idle cached shutdown retained a checkout owner" unless Db.open_statements(idle_conn).empty?
raise "idle cached shutdown forgot its connection" unless Db.instance_variable_get(:@quarantined).include?(idle_conn)
idle_cached.define_singleton_method(:close, real_idle_close)
Db.close
raise "idle cached shutdown retry leaked the statement" unless idle_cached.closed?
raise "idle cached shutdown retry retained quarantine" unless Db.instance_variable_get(:@quarantined).empty?
raise "idle cached shutdown retry retained the pool" unless Db.instance_variable_get(:@pool).nil?
puts "runtime: idle cached shutdown close failure and retry passed (8 assertions)"

# A checked-out statement's release error must not stop cached statement or
# connection disposal, including later connections with their own errors.
Db.configure(":memory:", pool_size: 3)
closing_conns = Db.instance_variable_get(:@pool).free.dup
closing_statements = []
first_shutdown_error = RuntimeError.new("first shutdown release failure")
closing_conns.each_with_index do |conn, index|
  Fiber[:db_handle] = conn
  idle = Db.prepare("SELECT 37 AS shutdown_idle")
  closing_statements.push(idle[:stmt])
  Db.finalize(idle)
  held = Db.prepare("SELECT 41 AS shutdown_held")
  transient = Db.prepare("SELECT 41 AS shutdown_held")
  closing_statements.push(held[:stmt], transient[:stmt])
  raise "missing shutdown readers" unless Db.step?(held) && Db.step?(transient)
  if index < 2
    error = index == 0 ? first_shutdown_error : RuntimeError.new("later shutdown release failure")
    held[:stmt].define_singleton_method(:reset!) { raise error }
  end
end
Fiber[:db_handle] = nil
shutdown_error = begin
  Db.close
  nil
rescue RuntimeError => e
  e
end
raise "shutdown leaked a cached or transient statement" unless closing_statements.all?(&:closed?)
raise "shutdown skipped a connection" unless closing_conns.all?(&:closed?)
raise "shutdown retained checkouts" unless closing_conns.all? { |conn| Db.open_statements(conn).empty? }
raise "shutdown retained its pool" unless Db.instance_variable_get(:@pool).nil?
raise "shutdown retained its owner" unless Db.instance_variable_get(:@owner_pid).nil?
raise "shutdown retained quarantine" unless Db.instance_variable_get(:@quarantined).empty?
raise "shutdown replaced its first release error" unless shutdown_error.equal?(first_shutdown_error)
Db.close
puts "runtime: gem shutdown drains all connections and preserves the first release error passed"
