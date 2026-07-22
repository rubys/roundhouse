module ActiveRecord
  # A lazy, chainable query builder — the metaprogramming-free analog of
  # ActiveRecord::Relation. Lowered model code drives it: `scope`s become
  # class methods that take/return a Relation, associations return one,
  # and a query chain (`Model.where(...).order(...).limit(...)`) is a
  # sequence of Relation method calls that only touches the database at a
  # terminal (`to_a`/`each`/`first`/`count`/…).
  #
  # No `method_missing`, no `define_method`: every method is written out.
  # The model is held as a plain class-object value (`@model`) whose
  # `table_name` / `instantiate` class methods supply the per-model facts;
  # calling them is ordinary dispatch.
  #
  # Database access and value escaping go through `ActiveRecord.adapter`
  # (the `AdapterInterface`) rather than the raw `Db` primitive, so the
  # whole class types against the same adapter contract `Base` uses — no
  # target-specific surface leaks in here. Chain methods mutate and return
  # `self`; lowered chains are linear (build then terminate), so a fresh
  # Relation per chain start is enough isolation.
  #
  # Terminals memoize: the first `to_a` loads and caches the records
  # (Rails' loaded-relation contract), so `map` + `each` + `empty?` on
  # the same relation hit the database once, not once per call — and
  # record mutations made between terminals (lobsters' current_vote
  # stamping) survive to the render. Every chain method drops the cache:
  # app code does re-chain after a terminal (`rel = rel.where(...)`
  # returns this same object, mutated), and a stale cache there would
  # serve the pre-refinement rows.
  class Relation
    def initialize(model)
      @model = model
      @table = model.table_name
      @wheres = []
      @joins = []
      @orders = []
      @groups = []
      @havings = []
      @select_sql = nil
      @distinct = false
      @limit = nil
      @offset = nil
      @includes = []
      @records = nil
    end

    # ---- chain methods (return self) --------------------------------

    # `where(hash)` / `where("raw sql")` / `where("a = ? AND b = ?", x, y)`.
    def where(condition = nil, *args)
      add_condition(condition, args, false)
      self
    end

    # `where.not(...)` is lowered to `not(...)` on the relation: negate the
    # condition back onto this relation.
    def not(condition = nil, *args)
      add_condition(condition, args, true)
      self
    end

    # `rel.or(other)` — Rails' Relation#or: this relation's accumulated
    # WHERE conjunction OR'd with the other's, grouped as one condition.
    # Rails requires matching structure (joins/limit) on both sides;
    # the receiver's structural clauses are kept here. An empty side is
    # an always-true condition (matches Rails: no filter to OR against).
    def or(other)
      @records = nil
      mine = @wheres.length > 0 ? @wheres.join(" AND ") : "1=1"
      other_wheres = other.where_clauses
      theirs = other_wheres.length > 0 ? other_wheres.join(" AND ") : "1=1"
      @wheres = ["((#{mine}) OR (#{theirs}))"]
      self
    end

    def add_condition(condition, args, negate)
      @records = nil
      return if condition.nil?
      sql = if condition.is_a?(Hash)
        hash_conditions(condition)
      else
        substitute_binds(condition.to_s, args)
      end
      return if sql == ""
      @wheres << (negate ? "NOT (#{sql})" : "(#{sql})")
      nil
    end

    def order(*parts)
      @records = nil
      parts.each { |p| @orders << order_term(p) }
      self
    end

    # Rails' mutating spellings. This Relation's chain methods already
    # mutate in place and return self, so the bang forms are the same
    # operation under a second name (bodies duplicated rather than
    # splat-forwarded — strict targets inline, not forward, rest args).
    def where!(condition = nil, *args)
      add_condition(condition, args, false)
      self
    end

    def order!(*parts)
      @records = nil
      parts.each { |p| @orders << order_term(p) }
      self
    end

    def limit(n)
      @records = nil
      @limit = n
      self
    end

    def offset(n)
      @records = nil
      @offset = n
      self
    end

    def group(*parts)
      @records = nil
      # Symbols qualify against this relation's table (Rails renders
      # `GROUP BY "tags"."id"`), so a grouped column stays unambiguous
      # once a join brings in a second table carrying the same column
      # name. Raw strings (expressions, pre-qualified columns) ride
      # verbatim.
      parts.each do |p|
        @groups << (p.is_a?(Symbol) ? "#{@table}.#{p}" : p.to_s)
      end
      self
    end

    def having(condition, *args)
      @records = nil
      @havings << substitute_binds(condition.to_s, args)
      self
    end

    def joins(spec)
      @records = nil
      @joins << spec.to_s
      self
    end

    def left_outer_joins(spec)
      @records = nil
      @joins << spec.to_s
      self
    end

    # `left_joins` — Rails alias for `left_outer_joins`.
    def left_joins(spec)
      left_outer_joins(spec)
    end

    # `select(:id, :username, "raw AS x")` — Symbols qualify against this
    # relation's table (as Rails renders them); raw strings ride verbatim.
    def select(*specs)
      @records = nil
      cols = []
      specs.each do |spec|
        cols << (spec.is_a?(Symbol) ? "#{@table}.#{spec}" : spec.to_s)
      end
      @select_sql = cols.join(", ")
      self
    end

    def distinct
      @records = nil
      @distinct = true
      self
    end

    # `includes`/`preload`/`eager_load` — eager-load hints. The specs
    # (Symbols, or Hashes for nested includes like `story: :user`) are
    # recorded here and executed by `to_a`, which hands them to the
    # model's synthesized `preload_associations` (batched `IN` loads
    # into the `_preload_<assoc>` caches). Models without a synthesized
    # override inherit Base's no-op and stay lazy (correct, just N+1).
    def includes(*names)
      @records = nil
      names.each { |n| @includes << n }
      self
    end

    def preload(*names)
      @records = nil
      names.each { |n| @includes << n }
      self
    end

    def eager_load(*names)
      @records = nil
      names.each { |n| @includes << n }
      self
    end

    # `references(:assoc)` — Rails' marker that a string condition
    # mentions an eager-loaded table. The preload machinery here decides
    # what to load from `eager_load`/`includes` alone, so the marker
    # carries no state; accept and ignore it so marker-bearing chains
    # stay chainable (lobsters' filters page).
    def references(*names)
      names
      self
    end

    # `merge(other)` — fold another relation's WHEREs in. v1 handles the
    # common case (merging a same-table scope's conditions).
    def merge(other)
      @records = nil
      other.where_clauses.each { |w| @wheres << w }
      self
    end

    def where_clauses
      @wheres
    end

    # `rel.arel` — the relation reified as its SELECT text, for the
    # `.arel.exists` correlated-subquery idiom
    # (`where.not(HiddenStory.….arel.exists)`). Captures the SQL at the
    # call: later chain mutations don't flow into it (matches lowered
    # usage, where `.arel` ends its chain).
    def arel
      Arel::SelectManager.new(to_sql)
    end

    def none
      @records = nil
      @wheres << "(1 = 0)"
      self
    end

    # `reload` — drop the loaded records; the next terminal re-queries and
    # reflects committed changes.
    def reload
      @records = nil
      self
    end

    # Rails' `load` — force the query now and memoize the records.
    def load
      to_a
      self
    end

    # The model class this relation queries — Rails' Relation#klass
    # (lobsters' Search switches on it to pick per-model joins).
    def klass
      @model
    end

    # ---- terminals --------------------------------------------------

    # Loads once, then serves the memoized records (see the class
    # comment). Hands back a shallow copy each call — Rails' `to_a`
    # contract — so a caller sorting or appending to the result can't
    # corrupt the cache; the record objects themselves stay shared.
    def to_a
      cached = @records
      return cached.dup unless cached.nil?
      rows = ActiveRecord.adapter.select_rows(to_sql)
      records = rows.map { |row| @model.instantiate(row) }
      @model.preload_associations(records, @includes) if @includes.length > 0
      @records = records
      records.dup
    end

    # Implicit array conversion — Rails delegates `to_ary` to the
    # loaded records, which is what lets `[story, relation].flatten`
    # splice the relation's records into the surrounding Array
    # (Array#flatten recurses into elements that respond to to_ary).
    def to_ary
      to_a
    end

    # `filter { |r| … }` / `select { |r| … }`-with-block are Enumerable
    # on the loaded records. (`select(*cols)` — the projection form —
    # is the separate chain method above; lowered call sites use
    # `filter` for the block form, matching the corpus.)
    def filter
      out = []
      to_a.each { |x| out << x if yield x }
      out
    end

    # `relation + array` — Rails materializes and concatenates
    # (`to_a + other`), yielding a plain Array. The set operations
    # (`&`, `|`, `-`) are delegated to the loaded records the same
    # way (ActiveRecord::Delegation's array-method delegation);
    # lobsters intersects `story.tags & filtered_tags`.
    def +(other)
      to_a + other
    end

    def &(other)
      to_a & other
    end

    def |(other)
      to_a | other
    end

    def -(other)
      to_a - other
    end

    # `include?(record)` — Rails checks membership against the loaded
    # records (`load` then id-compare); materializing matches that
    # contract at our result-set sizes.
    def include?(record)
      to_a.include?(record)
    end

    def each
      to_a.each { |x| yield x }
    end

    # `index_by { |r| key }` — the records as a Hash keyed by the
    # block's value, last write winning on duplicates (Rails'
    # contract; lobsters keys tag filters by id).
    def index_by
      h = {}
      to_a.each { |x| h[yield x] = x }
      h
    end

    # `find_each` — Rails batches in groups of 1000; the result set sizes
    # this runtime serves make plain iteration the same observable
    # behavior (ordering aside, which our callers don't rely on).
    def find_each
      to_a.each { |x| yield x }
    end

    def map
      to_a.map { |x| yield x }
    end

    # `group_by { |rec| key }` — Enumerable's grouping over the
    # materialized rows (lobsters threads comments with
    # `@comments.group_by(&:parent_comment_id)`). fetch-then-insert
    # rather than `Hash.new { [] }` (no default-proc portability) or
    # `[]=`-chaining on a maybe-missing key.
    def group_by
      out = {}
      to_a.each do |rec|
        k = yield rec
        arr = out.fetch(k, nil)
        if arr.nil?
          arr = []
          out[k] = arr
        end
        arr << rec
      end
      out
    end

    # `inject(initial) { |acc, x| ... }` — the accumulator form the
    # corpus uses (vote-hash batchers). The no-initial and Symbol forms
    # aren't modeled; callers pass an explicit seed.
    def inject(initial)
      acc = initial
      to_a.each { |x| acc = yield(acc, x) }
      acc
    end

    def first
      @limit = 1
      rows = to_a
      rows.length == 0 ? nil : rows[0]
    end

    # `first!` — like `first`, but raises `RecordNotFound` (→ 404 in the
    # dispatch layer) instead of returning nil when the relation is empty.
    def first!
      record = first
      raise RecordNotFound, "Couldn't find record in #{@table}" if record.nil?
      record
    end

    def last
      rows = to_a
      rows.length == 0 ? nil : rows[rows.length - 1]
    end

    def count
      rows = ActiveRecord.adapter.select_rows(count_sql)
      rows.length == 0 ? 0 : rows[0]["n"].to_i
    end

    # `sum(:col)` / `sum("<sql expr>")` — SQL SUM over the relation.
    # Returns Float: both corpus consumers are float arithmetic
    # (lobsters' hotness math); an Integer-column caller would want
    # column typing here, ledgered when one appears.
    def sum(expr)
      term = expr.is_a?(Symbol) ? "#{@table}.#{expr}" : expr.to_s
      sql = "SELECT COALESCE(SUM(#{term}), 0) AS n FROM #{@table}"
      sql = "#{sql} #{@joins.join(" ")}" if @joins.length > 0
      sql = "#{sql} WHERE #{@wheres.join(" AND ")}" if @wheres.length > 0
      rows = ActiveRecord.adapter.select_rows(sql)
      rows.length == 0 ? 0.0 : rows[0]["n"].to_f
    end

    # `group(:col).count` — Rails hands back a Hash of group-key =>
    # COUNT. The group_count lowering renames the grouped chain's
    # terminal to this method, so the scalar `count` keeps its
    # Integer return (no polymorphic count). Single group expression
    # (the corpus shape); Rails' multi-group array keys would need a
    # composite key here first.
    def group_count
      key = @groups.join(", ")
      sql = "SELECT #{key} AS k, COUNT(*) AS n FROM #{@table}"
      sql = "#{sql} #{@joins.join(" ")}" if @joins.length > 0
      sql = "#{sql} WHERE #{@wheres.join(" AND ")}" if @wheres.length > 0
      sql = "#{sql} GROUP BY #{key}"
      h = {}
      rows = ActiveRecord.adapter.select_rows(sql)
      rows.each { |row| h[row["k"]] = row["n"].to_i }
      h
    end

    # Loaded relations answer from the cache; unloaded ones keep the
    # COUNT round-trip (Rails asks EXISTS here — one row either way).
    def empty?
      r = @records
      r.nil? ? count == 0 : r.length == 0
    end

    def any?
      count > 0
    end

    # Block form of Enumerable#all? over the materialized rows (the
    # runtime `Base.where` fallback returns a Relation, and dynamic
    # call-sites treat it as the array Rails hands back).
    def all?
      ok = true
      to_a.each { |x| ok = false unless yield x }
      ok
    end

    def exists?
      count > 0
    end

    def length
      to_a.length
    end

    def size
      to_a.length
    end

    # `delete_all` — bulk DELETE scoped by the accumulated WHEREs.
    # Rails contract: no callbacks, no per-row loads, returns the
    # affected-row count. ORDER/LIMIT don't apply to bulk ops.
    def delete_all
      sql = "DELETE FROM #{@table}"
      sql = "#{sql} WHERE #{@wheres.join(" AND ")}" if @wheres.length > 0
      ActiveRecord.adapter.execute_ddl(sql)
      ActiveRecord.adapter.changes
    end

    # `update_all(...)` — bulk UPDATE scoped by the accumulated WHEREs.
    # Hash form (`update_all(user_id: 3)`) escapes values; String form
    # (`update_all("hits = hits + 1")`) is trusted verbatim, same as
    # Rails. Returns the affected-row count.
    def update_all(updates)
      set_sql = if updates.is_a?(Hash)
        parts = []
        updates.each do |key, val|
          parts.push("#{key} = #{ActiveRecord.adapter.escape_value(val)}")
        end
        parts.join(", ")
      else
        updates.to_s
      end
      sql = "UPDATE #{@table} SET #{set_sql}"
      sql = "#{sql} WHERE #{@wheres.join(" AND ")}" if @wheres.length > 0
      ActiveRecord.adapter.execute_ddl(sql)
      ActiveRecord.adapter.changes
    end

    # `pluck(:col)` — a single column projected to an Array of its raw
    # values (strings as stored; callers coerce).
    def pluck(col)
      @select_sql = "#{@table}.#{col} AS v"
      rows = ActiveRecord.adapter.select_rows(to_sql)
      rows.map { |row| row["v"] }
    end

    # `ids` — primary keys, as integers.
    def ids
      @select_sql = "#{@table}.id AS v"
      rows = ActiveRecord.adapter.select_rows(to_sql)
      rows.map { |row| row["v"].to_i }
    end

    def find(id)
      @wheres << "#{@table}.id = #{ActiveRecord.adapter.escape_value(id)}"
      first
    end

    def find_by(conditions)
      add_condition(conditions, [], false)
      first
    end

    # `find_by!` — `find_by` that raises `RecordNotFound` on no match.
    def find_by!(conditions)
      record = find_by(conditions)
      raise RecordNotFound, "Couldn't find record in #{@table}" if record.nil?
      record
    end

    # `first_or_initialize` — the first matching row, or a new unsaved
    # record when there is none. The caller assigns the remaining
    # attributes before `save` (the write path this serves), so the built
    # record starts blank rather than pre-filled from the where-conditions.
    def first_or_initialize
      record = first
      record.nil? ? @model.new : record
    end

    # ---- SQL composition --------------------------------------------

    def to_sql
      cols = @select_sql.nil? ? "#{@table}.*" : @select_sql
      distinct = @distinct ? "DISTINCT " : ""
      sql = "SELECT #{distinct}#{cols} FROM #{@table}"
      sql = "#{sql} #{@joins.join(" ")}" if @joins.length > 0
      sql = "#{sql} WHERE #{@wheres.join(" AND ")}" if @wheres.length > 0
      sql = "#{sql} GROUP BY #{@groups.join(", ")}" if @groups.length > 0
      sql = "#{sql} HAVING #{@havings.join(" AND ")}" if @havings.length > 0
      sql = "#{sql} ORDER BY #{@orders.join(", ")}" if @orders.length > 0
      sql = "#{sql} LIMIT #{@limit}" unless @limit.nil?
      sql = "#{sql} OFFSET #{@offset}" unless @offset.nil?
      sql
    end

    def count_sql
      sql = "SELECT COUNT(*) AS n FROM #{@table}"
      sql = "#{sql} #{@joins.join(" ")}" if @joins.length > 0
      sql = "#{sql} WHERE #{@wheres.join(" AND ")}" if @wheres.length > 0
      sql
    end

    # ---- helpers ----------------------------------------------------

    # A hash of conditions ANDed: `{is_deleted: false, user_id: 3}` ->
    # `is_deleted = 0 AND user_id = 3`. Array value -> `IN`, nil ->
    # `IS NULL`, nested Hash -> qualified `table.col = ...`.
    def hash_conditions(hash)
      parts = []
      hash.each do |key, val|
        if val.is_a?(Hash)
          val.each do |col, v|
            parts << column_predicate("#{key}.#{col}", v)
          end
        else
          parts << column_predicate(key.to_s, val)
        end
      end
      parts.join(" AND ")
    end

    # Unqualified columns are qualified with this relation's own table
    # (as Rails does for hash conditions) so a condition survives `merge`
    # into a JOINed query where the bare name would be ambiguous —
    # `hidden_stories.user_id`, not `user_id`, after `joins(:hidings)`.
    def column_predicate(col, val)
      qcol = col.include?(".") ? col : "#{@table}.#{col}"
      if val.is_a?(Relation)
        # A relation value is Rails' subquery form —
        # `where(story_id: Tagging.where(...).select(:story_id))` →
        # `story_id IN (SELECT taggings.story_id FROM taggings …)`.
        # The inner relation renders inline; its values were escaped
        # as its own conditions were added.
        "#{qcol} IN (#{val.to_sql})"
      elsif val.is_a?(Array)
        "#{qcol} IN (#{escape_list(val)})"
      elsif val.nil?
        "#{qcol} IS NULL"
      else
        "#{qcol} = #{ActiveRecord.adapter.escape_value(val)}"
      end
    end

    # Replace `?` placeholders in a raw fragment with escaped args, in
    # order. A fragment with no `?` returns unchanged. Each `sub` rewrites
    # the leftmost remaining `?`, so iterating the args consumes them in
    # order.
    def substitute_binds(sql, args)
      result = sql
      args.each { |a| result = result.sub("?", ActiveRecord.adapter.escape_value(a)) }
      result
    end

    def escape_list(vals)
      out = []
      vals.each { |v| out << ActiveRecord.adapter.escape_value(v) }
      out.join(", ")
    end

    # `order(:col)` / `order("col DESC")` / `order(col: :desc)`.
    def order_term(p)
      if p.is_a?(Hash)
        parts = []
        p.each { |col, dir| parts << "#{col} #{dir.to_s.upcase}" }
        parts.join(", ")
      else
        p.to_s
      end
    end
  end
end
