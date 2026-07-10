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

    def add_condition(condition, args, negate)
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
      parts.each { |p| @orders << order_term(p) }
      self
    end

    def limit(n)
      @limit = n
      self
    end

    def offset(n)
      @offset = n
      self
    end

    def group(*parts)
      parts.each { |p| @groups << p.to_s }
      self
    end

    def having(condition, *args)
      @havings << substitute_binds(condition.to_s, args)
      self
    end

    def joins(spec)
      @joins << spec.to_s
      self
    end

    def left_outer_joins(spec)
      @joins << spec.to_s
      self
    end

    # `select(:id, :username, "raw AS x")` — Symbols qualify against this
    # relation's table (as Rails renders them); raw strings ride verbatim.
    def select(*specs)
      cols = []
      specs.each do |spec|
        cols << (spec.is_a?(Symbol) ? "#{@table}.#{spec}" : spec.to_s)
      end
      @select_sql = cols.join(", ")
      self
    end

    def distinct
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
      names.each { |n| @includes << n }
      self
    end

    def preload(*names)
      names.each { |n| @includes << n }
      self
    end

    def eager_load(*names)
      names.each { |n| @includes << n }
      self
    end

    # `merge(other)` — fold another relation's WHEREs in. v1 handles the
    # common case (merging a same-table scope's conditions).
    def merge(other)
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
      @wheres << "(1 = 0)"
      self
    end

    # ---- terminals --------------------------------------------------

    def to_a
      rows = ActiveRecord.adapter.select_rows(to_sql)
      records = rows.map { |row| @model.instantiate(row) }
      @model.preload_associations(records, @includes) if @includes.length > 0
      records
    end

    def each
      to_a.each { |x| yield x }
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

    # `last` — Rails reverses the order and takes ONE row, not a full
    # materialize + `[-1]`. The unordered case (the hot one — `Model.last`
    # routes here) becomes `ORDER BY <pk> DESC LIMIT 1` via `first`. An
    # explicit order would need per-term reversal; those call sites are
    # paginated/small, so they keep the materialize fallback rather than
    # risk mis-reversing a compound order.
    def last
      if @orders.length == 0
        @orders << order_term("#{@table}.id DESC")
        return first
      end
      rows = to_a
      rows.length == 0 ? nil : rows[rows.length - 1]
    end

    def count
      rows = ActiveRecord.adapter.select_rows(count_sql)
      rows.length == 0 ? 0 : rows[0]["n"].to_i
    end

    def empty?
      count == 0
    end

    def any?
      count > 0
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
      if val.is_a?(Array)
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
