# frozen_string_literal: true
require_relative "../test_helper"
require "active_record/arel"

# Arel nodes render to the SQL text Relation interpolates. Binds are
# filled by Arel.sql: a node is embedded, a scalar is quoted.
class ArelNodesTest < Minitest::Test
  def test_sql_without_binds_is_the_fragment
    assert_equal "SUM(energy_kwh)", Arel.sql("SUM(energy_kwh)").to_s
    assert_instance_of Arel::Nodes::SqlLiteral, Arel.sql("SUM(energy_kwh)")
  end

  def test_named_bind_quotes_a_scalar_and_embeds_a_node
    quoted = Arel::Nodes.build_quoted("era's")
    assert_equal "'era''s'", quoted.to_s
    sql = Arel.sql("date < :boundary AND :flag", boundary: quoted, flag: true)
    assert_equal "date < 'era''s' AND 1", sql.to_s
  end

  def test_a_shorter_name_does_not_eat_a_longer_one
    sql = Arel.sql(":era_boundary > :era", era_boundary: 2, era: 1)
    assert_equal "2 > 1", sql.to_s
  end

  def test_a_cast_is_not_a_named_bind
    sql = Arel.sql("(period)::date = :date", date: "2024-01-01")
    assert_equal "(period)::date = '2024-01-01'", sql.to_s
  end

  def test_a_placeholder_inside_a_string_literal_stays
    sql = Arel.sql("note = 'keep :name' AND name = :name", name: "Ada")
    assert_equal "note = 'keep :name' AND name = 'Ada'", sql.to_s
  end

  def test_positional_question_mark
    sql = Arel.sql("id = ? AND label = ?", 4, "a'b")
    assert_equal "id = 4 AND label = 'a''b'", sql.to_s
  end

  def test_a_positional_node_keeps_its_own_question_mark
    sql = Arel.sql("id = ? AND label = ?", Arel.sql("COALESCE(name, ?)"), 7)
    assert_equal "id = COALESCE(name, ?) AND label = 7", sql.to_s
    quoted = Arel.sql("note = '?' AND id = ?", 4)
    assert_equal "note = '?' AND id = 4", quoted.to_s
  end

  def test_not_case_and_named_function
    predicate = "active = 1"
    expr = Arel::Nodes::Not.new(predicate)
    assert_equal "NOT (active = 1)", expr.to_s
    cse = Arel::Nodes::Case.new.when(Arel.sql(predicate)).then(1).else(nil)
    assert_equal "CASE WHEN active = 1 THEN 1 ELSE NULL END", cse.to_s
    quoted = Arel::Nodes::Case.new.when("active = 1").then("high").else("low")
    assert_equal "CASE WHEN 'active = 1' THEN 'high' ELSE 'low' END", quoted.to_s
    fn = Arel::Nodes::NamedFunction.new("COUNT", [cse])
    assert_equal "COUNT(CASE WHEN active = 1 THEN 1 ELSE NULL END)", fn.to_s
  end

  def test_unmatched_binds_are_refused
    assert_raises(Arel::BindError) { Arel.sql("id = ?", 1, 2) }
    assert_raises(Arel::BindError) { Arel.sql("id = ? AND label = ?", 1) }
    assert_raises(Arel::BindError) { Arel.sql("id = :id", name: 1) }
    sql = Arel.sql("id = :id", id: 1, extra: 2)
    assert_equal "id = 1", sql.to_s
  end

  def test_a_sql_literal_bind_is_embedded_not_quoted
    inner = Arel.sql("lost_kwh > :n", n: 0)
    outer = Arel.sql("SUM(CASE WHEN :scope THEN 1 ELSE 0 END)", scope: inner)
    assert_equal "SUM(CASE WHEN lost_kwh > 0 THEN 1 ELSE 0 END)", outer.to_s
  end
end
