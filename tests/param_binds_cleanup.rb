# Faults run inside the actual emitted serialization/bind/hydration methods.
# Every ownership assertion is inside the same lease as the rescued failure.
module CleanupFaults
  @current = ""
  def self.current=(value)
    @current = value
  end
  def self.trip(value)
    raise "injected cleanup " + value if value == @current
    nil
  end
end

def expect_cleanup_failure(fault)
  CleanupFaults.current = fault
  begin
    yield
    raise "missing cleanup failure " + fault
  rescue RuntimeError => error
    raise error unless error.message.include?("injected cleanup " + fault)
  ensure
    CleanupFaults.current = ""
  end
  count = Db.cleanup_owned_count
  raise fault + " left " + count.to_s + " owned statements inside the lease" unless count == 0
  nil
end

SqliteAdapter.configure("file:cleanup_gate?mode=memory&cache=shared")
ActiveRecord.adapter = SqliteAdapter
Schema.statements.each { |sql| Db.exec(sql) }
Db.exec("INSERT INTO parents (id, name) VALUES (1, 'one')")
Db.exec("INSERT INTO readings (id, parent_id, recorded_at, label) VALUES (1, 1, '2023-11-14 22:13:20.000000', 'one')")
needle = Time.at(1700000000).utc
row = Reading.find(1)
controller = ParentsController.new
binds = ENV["ROUNDHOUSE_PARAM_BINDS"] == "1"
Db.with_connection do
  3.times do
    expect_cleanup_failure("serialize") { row.time_count(needle) }
    expect_cleanup_failure("bind") { row.time_count(needle) } if binds
    expect_cleanup_failure("step") { row.time_count(needle) }
    expect_cleanup_failure("hydrate") { row.single(1) }
    expect_cleanup_failure("hydrate") { row.many(1) }
    expect_cleanup_failure("reload") { row._adapter_reload }
    expect_cleanup_failure("hydrate") { controller.preloaded }
    expect_cleanup_failure("hydrate") { Reading._hydrate_all("SELECT id, parent_id, recorded_at, label FROM readings") }
    raise "lease no longer usable" unless row.time_count(needle) == 1
    raise "successful recovery retained a statement" unless Db.cleanup_owned_count == 0
  end
  true
end
puts "cleanup: " + (binds ? "24" : "21") + " rescued generated failures; every owned count was zero before lease end"
Db.close
