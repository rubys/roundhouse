# Arel — the raw-SQL corner of ActiveRecord's query surface, sized to the
# string-composed Relation runtime: a "fragment" here IS its SQL text.
# `Arel.sql(...)` marks a caller-authored fragment (identity — Relation
# already treats string conditions as raw SQL), and `Relation#arel` wraps
# the relation's SELECT so `.exists` can splice it into an enclosing
# WHERE as a correlated subquery (`where.not(<rel>.arel.exists)`).
module Arel
  def self.sql(fragment)
    fragment
  end

  # A relation's SELECT reified as its SQL text. Mirrors the role (not
  # the structure) of Arel::SelectManager.
  class SelectManager
    def initialize(sql)
      @sql = sql
    end

    def to_sql
      @sql
    end

    def exists
      "EXISTS (#{@sql})"
    end
  end
end
