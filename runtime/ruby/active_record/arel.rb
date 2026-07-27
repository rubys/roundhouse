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

  # The `*` projection, for `arel_table.project(Arel.star)`.
  def self.star
    "*"
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

  # A table reference (`Model.arel_table`). `attribute` yields the
  # qualified column as an Attribute; `project` renders a projection
  # qualified against this table (`stories.*` for `Arel.star`).
  #
  # Rails spells the reader `[]`, and `src/lower/arel_attribute.rs`
  # lowers that spelling to this name — an indexer def here would put an
  # arm for a by-value class into spinel's poly `[]` dispatcher and
  # break every unrelated `x[k]` in the program. Rails' `project` is
  # variadic; the single-column form is the one the corpus calls and the
  # one a strict target can type — a splat parameter has no
  # cross-target-safe shape.
  class Table
    attr_reader :name

    def initialize(name)
      @name = name
    end

    def attribute(column)
      Attribute.new("#{@name}.#{column}")
    end

    def project(column)
      "#{@name}.#{column}"
    end
  end

  # A qualified column (`stories.id`). Predicates render to SQL
  # fragments; the subquery operand is anything with `to_sql`
  # (`Relation#arel`'s SelectManager, or a Relation itself).
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
  # Reopened here rather than in `base.rb`: `base.rb` is transpiled into
  # every strict target through the runtime_loader tables, and an
  # `Arel::Table` return type would drag this whole file in with it.
  # `arel.rb` is ruby-family-only, so the reopen — required after
  # `base.rb`, the same ordering contract `connection.rb` relies on —
  # keeps the Arel surface where the targets that have it can see it.
  class Base
    # Class-method inheritance reaches every model; `table_name` is the
    # per-model fact the synthesized models already supply.
    def self.arel_table
      Arel::Table.new(table_name)
    end
  end
end
