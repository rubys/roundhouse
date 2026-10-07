require_relative "boot"
require_relative "app/models/indexed_row"
require_relative "app/models/paired_row"
require_relative "app/models/joined_row"
require_relative "app/models/wide_row"
SqliteAdapter.configure(":memory:")
ActiveRecord.adapter = SqliteAdapter
Schema.statements.each { |sql| Db.exec(sql) }

# The Arel fast path does not yet fold joins. A view lets the emitted
# nullable WHERE predicate participate in SQLite's real LEFT JOIN planner
# without adding unsupported joins to the lowerer or rewriting emitted SQL.
Db.exec("DROP TABLE joined_rows")
Db.exec("CREATE VIEW joined_rows AS SELECT host_rows.id, indexed_rows.a FROM host_rows LEFT JOIN indexed_rows ON indexed_rows.id = host_rows.id")
Db.exec("WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id + 1 FROM n WHERE id < 1000) INSERT INTO indexed_rows SELECT id, CASE WHEN id = 7 THEN 7 END FROM n")
Db.exec("WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id + 1 FROM n WHERE id < 1000) INSERT INTO paired_rows SELECT id, CASE WHEN id = 7 THEN 7 END, CASE WHEN id = 7 THEN 11 END FROM n")
Db.exec("INSERT INTO host_rows SELECT id FROM indexed_rows")
Db.exec("INSERT INTO host_rows VALUES (1001)")
Db.exec("INSERT INTO wide_rows DEFAULT VALUES")
Db.exec("CREATE INDEX single_present ON indexed_rows(a) WHERE a IS NOT NULL")
Db.exec("CREATE INDEX pair_present ON paired_rows(a, b) WHERE a IS NOT NULL AND b IS NOT NULL")
Db.exec("ANALYZE")

module Db
  class << self
    attr_reader :planner_sql, :planner_binds
    alias planner_original_prepare prepare
    alias planner_original_prepare_uncached prepare_uncached
    alias planner_original_bind_value bind_value
    def prepare(sql)
      @planner_sql = sql
      @planner_binds = []
      planner_original_prepare(sql)
    end
    def prepare_uncached(sql)
      @planner_sql = sql
      @planner_binds = []
      planner_original_prepare_uncached(sql)
    end
    def bind_value(handle, index, value)
      @planner_binds[index - 1] = value
      planner_original_bind_value(handle, index, value)
    end
    def planner_details
      current_dbh.execute("EXPLAIN QUERY PLAN " + @planner_sql, @planner_binds).map { |row| row[3] }.join("; ")
    end
  end
end

def expect_result(label, expected, actual)
  raise "#{label}: expected #{expected}, got #{actual}" unless expected == actual
end

bound = ENV.fetch("PLANNER_BINDS") == "1"
single = IndexedRow.new
pair = PairedRow.new
joined = JoinedRow.new
Db.with_connection do
  single_shapes = []
  pair_shapes = []
  3.times do
    expect_result("single non-nil", 1, single.matching(7))
    single_shapes << Db.planner_sql
    plan = Db.planner_details
    raise "single partial index lost: #{Db.planner_sql}: #{plan}" unless plan.include?("USING COVERING INDEX single_present (a=?)")
    expect_result("single placeholder values", bound ? [7] : [], Db.planner_binds)
    puts "planner binds=#{bound}: single partial index: #{plan}"

    expect_result("single nil", 999, single.matching(nil))
    single_shapes << Db.planner_sql
    raise "nil must remove bind slot: #{Db.planner_sql}" unless Db.planner_sql.include?("a IS NULL") && !Db.planner_sql.include?("?")
    expect_result("single nil bind count", [], Db.planner_binds)

    expect_result("pair non-nil", 1, pair.matching(7, 11))
    pair_shapes << Db.planner_sql
    plan = Db.planner_details
    raise "composite partial index lost: #{Db.planner_sql}: #{plan}" unless plan.include?("USING COVERING INDEX pair_present (a=? AND b=?)")
    expect_result("pair placeholder order", bound ? [7, 11] : [], Db.planner_binds)
    puts "planner binds=#{bound}: composite partial index: #{plan}"

    expect_result("pair nil/non-nil", 0, pair.matching(nil, 11))
    pair_shapes << Db.planner_sql
    expect_result("pair removed first slot", bound ? [11] : [], Db.planner_binds)
    expect_result("pair non-nil/nil", 0, pair.matching(7, nil))
    pair_shapes << Db.planner_sql
    expect_result("pair removed last slot", bound ? [7] : [], Db.planner_binds)
    expect_result("pair nil/nil", 999, pair.matching(nil, nil))
    pair_shapes << Db.planner_sql
    expect_result("pair no slots", [], Db.planner_binds)

    expect_result("joined non-nil", 1, joined.matching(7))
    plan = Db.planner_details
    raise "LEFT JOIN was not strength-reduced: #{Db.planner_sql}: #{plan}" if plan.include?("LEFT-JOIN")
    raise "reduced join lost partial index: #{plan}" unless plan.include?("single_present")
    puts "planner binds=#{bound}: LEFT JOIN strength reduction: #{plan}"

    expect_result("joined nil including unmatched row", 1000, joined.matching(nil))
    plan = Db.planner_details
    raise "nil must retain LEFT JOIN: #{plan}" unless plan.include?("LEFT-JOIN")
    expect_result("join nil bind count", [], Db.planner_binds)
  end
  expect_result("single shape count", 2, single_shapes.uniq.length)
  expect_result("pair shape count", 4, pair_shapes.uniq.length)
  # Changing the non-nil values must reuse the already observed bound
  # shape. Inline mode intentionally retains value-bearing main SQL.
  [8, 19, 1023].each do |value|
    expect_result("single changed bind #{value}", 0, single.matching(value))
    expect_result("single non-nil values share one shape", single_shapes[0], Db.planner_sql) if bound
    expect_result("pair changed bind #{value}", 0, pair.matching(value, value + 1))
    expect_result("pair non-nil values share one shape", pair_shapes[0], Db.planner_sql) if bound
  end

  wide = WideRow.new
  cache = Db.current_dbh.instance_variable_get(:@rh_stmt_cache)
  cached_before = cache.length
  256.times do |mask|
    values = 8.times.map { |bit| (mask & (1 << bit)) == 0 ? nil : bit + 1 }
    expect_result("eight-nullable shape #{mask}", mask == 0 ? 1 : 0, wide.matching(*values))
    expect_result("eight-nullable bind order #{mask}", bound ? values.compact : [], Db.planner_binds)
  end
  expect_result("eight nullable predicates bypass statement cache", cached_before, cache.length) if bound
end
puts "planner: binds=#{bound}, 3 repeated single/composite/join plan groups, 2/4 null patterns, varying non-nil values, 256 eight-nullable combinations passed"
Db.close
