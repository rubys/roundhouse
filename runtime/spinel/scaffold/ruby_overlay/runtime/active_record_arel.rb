# CRuby-only Arel::Table surface: `Model.arel_table[:col]` attributes,
# `not_in`/`in` subquery predicates, and `project(Arel.star)`.
#
# Extends the transpiled Arel shim (runtime/active_record/arel.rb), keeping
# its contract: a fragment IS its SQL text, so everything here renders to
# strings the Relation's where/group/select arms already accept verbatim.
#
# Lives on the CRuby overlay, not shared runtime/ruby: `Table#[]` is an
# indexer def — exactly the shape that broke the strict-target transpile of
# CookieJar's `[]=` (go emits `return m[k] = v`) — and only lobsters
# exercises this corner. When another target gets lobsters, it grows its
# own variant (likely `attribute(col)` plus an emit-side index lowering).
module Arel
  # The `*` projection, for `arel_table.project(Arel.star)`.
  def self.star
    "*"
  end

  # A table reference (`Model.arel_table`). `[]` yields the qualified
  # column as an Attribute; `project` renders a projection list qualified
  # against this table (`stories.*` for `Arel.star`).
  class Table
    attr_reader :name

    def initialize(name)
      @name = name
    end

    def [](column)
      Attribute.new("#{@name}.#{column}")
    end

    def project(*columns)
      columns.map { |c| "#{@name}.#{c}" }.join(", ")
    end
  end

  # A qualified column (`stories.id`). Predicates render to SQL fragments;
  # the subquery operand is anything with `to_sql` (`Relation#arel`'s
  # SelectManager, or a Relation itself).
  class Attribute
    def initialize(qualified)
      @qualified = qualified
    end

    def to_s
      @qualified
    end

    def in(subquery)
      "#{@qualified} IN (#{subquery.to_sql})"
    end

    def not_in(subquery)
      "#{@qualified} NOT IN (#{subquery.to_sql})"
    end
  end
end

module ActiveRecord
  class Base
    # Class-method inheritance reaches every model; `table_name` is the
    # per-model fact the synthesized models already supply.
    def self.arel_table
      Arel::Table.new(table_name)
    end
  end
end
