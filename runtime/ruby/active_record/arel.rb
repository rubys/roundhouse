# frozen_string_literal: true
# Arel — the raw-SQL corner of ActiveRecord's query surface, sized to the
# string-composed Relation runtime: a "fragment" here IS its SQL text.
# `Arel.sql(...)` marks a caller-authored fragment (identity — Relation
# already treats string conditions as raw SQL), and `Relation#arel` wraps
# the relation's SELECT so `.exists` can splice it into an enclosing
# WHERE as a correlated subquery (`where.not(<rel>.arel.exists)`).
module Arel
  # A bind that does not match the fragment. Rails raises this from
  # `Arel.sql` before the SQL is handed to the connection. Its parent is
  # Rails' (arel/errors.rb), so `rescue Arel::ArelError` catches it.
  class ArelError < StandardError
  end

  class BindError < ArelError
  end

  # A caller-authored fragment. With no binds this is that text, wrapped
  # so a later bind can tell it from a value that still needs quoting
  # (`scope_evidence: Arel.sql(...)`). Named binds (`:key`) and positional
  # `?` marks are filled here: a node is its SQL, anything else is a
  # quoted literal. Rails keeps the binds for the connection; this
  # runtime's relation is the SQL text, so the substitution happens now.
  def self.sql(fragment, *positional, **named)
    text = fragment.to_s
    if !positional.empty? && Nodes.count_questions(text) != positional.length
      raise BindError, "wrong number of bind variables (#{positional.length} for #{Nodes.count_questions(text)})"
    end
    # One pass over the original fragment. A bind can be a node whose
    # own SQL still contains `?` (a bind-free `Arel.sql` keeps that
    # text); scanning the already-mutated string would fill those.
    unless positional.empty?
      text = Nodes.replace_questions(text, positional.map { |bind| Nodes.embed(bind) })
    end
    names = named.keys.map(&:to_s).sort_by { |key| -key.length }
    names.each do |key|
      text = Nodes.replace_named(text, key, Nodes.embed(named[key.to_sym]))
    end
    if !positional.empty? || !named.empty?
      missing = Nodes.unbound_name(text)
      raise BindError, "missing value for :#{missing}" unless missing.nil?
    end
    Nodes::SqlLiteral.new(text)
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

    # The text carries the relation's bind marks so `exists` can splice
    # them into an enclosing read; printed, the values are written in.
    def to_sql
      ActiveRecord::Relation.inline_sql(@sql)
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

  # SQL AST as text. A node renders itself with `to_s` / `to_sql`;
  # `Relation` already interpolates an argument that way (`where`,
  # `select`). Not a String subclass: the runtime does not subclass
  # builtins, and `to_s` is the conversion those call sites use.
  module Nodes
    def self.embed(value)
      if value.is_a?(Node)
        value.to_s
      else
        quote_scalar(value)
      end
    end

    # A SQL literal. nil, numbers, and booleans stay bare: a boolean is
    # the database's literal (SQLite's 1, PostgreSQL's TRUE), as `Db`
    # writes it. Strings go through `Db.escape_string`, the same literal
    # `Connection#quote` used to build itself, so a NUL or a binary value
    # stays one literal.
    def self.quote_scalar(value)
      return "NULL" if value.nil?
      return value.to_s if value.is_a?(Integer) || value.is_a?(Float)
      return Db.escape_bool_opt(true) if value == true
      return Db.escape_bool_opt(false) if value == false
      Db.escape_string(value.to_s)
    end

    # `Arel::Nodes.build_quoted(value)` — the quoted form of one value,
    # already safe to embed in a fragment.
    def self.build_quoted(value)
      Quoted.new(quote_scalar(value))
    end

    # The first `?` outside a single-quoted string.
    def self.replace_question(text, literal)
      replace_questions(text, [literal])
    end

    # Each `?` outside a single-quoted string, in order. Replacement
    # text is copied as-is, so a `?` inside an embedded fragment stays.
    def self.replace_questions(text, literals)
      out = ""
      i = 0
      bind_index = 0
      quoted = false
      while i < text.length
        ch = text[i]
        if ch == "'"
          quoted = !quoted
          out += ch
        elsif !quoted && ch == "?" && bind_index < literals.length
          out += literals[bind_index]
          bind_index += 1
        else
          out += ch
        end
        i += 1
      end
      out
    end

    # `:key` at a name boundary, outside a single-quoted string.
    # Callers replace longer names first, so `:era` does not eat
    # `:era_boundary`.
    def self.replace_named(text, key, literal)
      mark = ":" + key
      n = mark.length
      out = ""
      i = 0
      quoted = false
      while i < text.length
        ch = text[i]
        prev = i > 0 ? text[i - 1] : ""
        if ch == "'"
          quoted = !quoted
          out += ch
          i += 1
        elsif !quoted && prev != ":" && text[i, n] == mark && name_boundary?(text[i + n, 1])
          out += literal
          i += n
        else
          out += ch
          i += 1
        end
      end
      out
    end

    def self.name_boundary?(ch)
      return true if ch.nil? || ch == ""
      ch.match?(/[^0-9A-Za-z_]/)
    end

    # `?` marks outside a single-quoted string. A bind count has to
    # match this before `sql` will build a literal.
    def self.count_questions(text)
      count = 0
      i = 0
      quoted = false
      while i < text.length
        ch = text[i]
        if ch == "'"
          quoted = !quoted
        elsif !quoted && ch == "?"
          count += 1
        end
        i += 1
      end
      count
    end

    # The first `:name` still in `text` after substitution, outside a
    # string and not the second colon of a PostgreSQL cast.
    def self.unbound_name(text)
      i = 0
      quoted = false
      while i < text.length
        ch = text[i]
        prev = i > 0 ? text[i - 1] : ""
        nxt = text[i + 1, 1]
        if ch == "'"
          quoted = !quoted
          i += 1
        elsif !quoted && ch == ":" && prev != ":" && name_start?(nxt)
          j = i + 1
          j += 1 while j < text.length && !name_boundary?(text[j, 1])
          return text[i + 1, j - i - 1]
        else
          i += 1
        end
      end
      nil
    end

    def self.name_start?(ch)
      return false if ch.nil? || ch == ""
      ch.match?(/[A-Za-z_]/)
    end

    class Node
      def to_s
        ""
      end

      def to_sql
        to_s
      end
    end

    class SqlLiteral < Node
      def initialize(sql)
        @sql = sql.to_s
      end

      def to_s
        @sql
      end
    end

    class Quoted < Node
      def initialize(sql)
        @sql = sql.to_s
      end

      def to_s
        @sql
      end
    end

    class Not < Node
      def initialize(expr)
        @expr = expr.to_s
      end

      def to_s
        "NOT (#{@expr})"
      end
    end

    # `Case.new.when(pred).then(value)` — each pair is one WHEN, and
    # `else` is optional. `when` / `then` / `else` return the case so
    # they chain. A node renders as its SQL. Every other value is
    # quoted the way `Nodes.quote_scalar` quotes a bind.
    class Case < Node
      def initialize
        @predicates = []
        @results = []
        @pending = nil
        @fallback = nil
      end

      def when(predicate)
        @pending = Case.fragment(predicate)
        self
      end

      def then(value)
        @predicates << @pending
        @results << Case.fragment(value)
        @pending = nil
        self
      end

      def else(value)
        @fallback = Case.fragment(value)
        self
      end

      def self.fragment(value)
        return value.to_s if value.is_a?(Node)
        Nodes.quote_scalar(value)
      end

      def to_s
        text = "CASE"
        i = 0
        while i < @predicates.length
          text = "#{text} WHEN #{@predicates[i]} THEN #{@results[i]}"
          i += 1
        end
        text = "#{text} ELSE #{@fallback}" unless @fallback.nil?
        "#{text} END"
      end
    end

    class NamedFunction < Node
      def initialize(name, exprs)
        @name = name.to_s
        @exprs = exprs.map { |expr| expr.to_s }
      end

      def to_s
        "#{@name}(#{@exprs.join(", ")})"
      end
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
