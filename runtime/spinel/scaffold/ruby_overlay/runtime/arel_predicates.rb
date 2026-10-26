# frozen_string_literal: true
# CRuby-only Arel predicates and ordering nodes: `attr.eq(v)`,
# `attr.matches(pattern)`, `attr.in([ids])`, `pred.or(other)`,
# `Arel::Nodes::Ascending.new(attr).nulls_first` — what ancestry's
# materialized path builds its scopes from.
#
# Lives on the CRuby overlay, not shared runtime/ruby/active_record/
# arel.rb: these nodes hand their SQL text on as Strings other nodes
# interpolate, and in Spinel's runtime that sharing stops `Arel.sql`'s
# bind substitution from compiling. Spinel's String handles are being
# redesigned; this moves there once they land.
module Arel
  class Attribute
    # A subquery (anything with `to_sql`), or a list of values, which an
    # empty list makes match nothing, as Rails renders it.
    def in(subquery)
      return "#{@qualified} IN (#{subquery.to_sql})" unless subquery.is_a?(Array)
      return "1=0" if subquery.empty?
      "#{@qualified} IN (#{subquery.map { |v| Nodes.embed(v) }.join(", ")})"
    end

    def eq(value)
      return Nodes::Predicate.new("#{@qualified} IS NULL") if value.nil?
      Nodes::Predicate.new("#{@qualified} = #{Nodes.embed(value)}")
    end

    # Rails' SQLite and MySQL visitors render LIKE whatever
    # `case_sensitive` says; only PostgreSQL's turns it into ILIKE.
    def matches(pattern, escape = nil, _case_sensitive = false)
      sql = "#{@qualified} LIKE #{Nodes.embed(pattern)}"
      sql = "#{sql} ESCAPE #{Nodes.embed(escape)}" unless escape.nil?
      Nodes::Predicate.new(sql)
    end
  end

  module Nodes
    # A WHERE predicate (`attr.eq(v)`) that combines with another.
    class Predicate < Node
      def initialize(sql)
        @sql = sql
      end

      def to_s
        @sql
      end

      def or(other)
        Predicate.new("(#{@sql} OR #{other})")
      end

      def and(other)
        Predicate.new("(#{@sql} AND #{other})")
      end
    end

    # `Ascending.new(attr).nulls_first` — an ORDER BY term.
    class Ascending < Node
      def initialize(expr)
        @expr = expr.to_s
        @nulls = ""
      end

      def nulls_first
        @nulls = " NULLS FIRST"
        self
      end

      def nulls_last
        @nulls = " NULLS LAST"
        self
      end

      def to_s
        "#{@expr} ASC#{@nulls}"
      end
    end

    class Descending < Node
      def initialize(expr)
        @expr = expr.to_s
        @nulls = ""
      end

      def nulls_first
        @nulls = " NULLS FIRST"
        self
      end

      def nulls_last
        @nulls = " NULLS LAST"
        self
      end

      def to_s
        "#{@expr} DESC#{@nulls}"
      end
    end
  end
end
