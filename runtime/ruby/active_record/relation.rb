module ActiveRecord
  # A Relation's `includes`/`preload`, deferred until a record asks.
  #
  # `load_records` used to batch-load every included association the
  # moment the rows arrived. On a page whose records only feed a cached
  # collection (campfire's room page: 40 messages, their rich text,
  # creators, boosts and attachments), the cache key needs each record's
  # id and updated_at, and on a hit nothing reads an association at all:
  # the batch loads were most of the page's database time. Now each record
  # carries this object, and the first association read on any of them
  # (an emitted reader calls `_await_preload`) runs the same batched
  # preload for the whole group, once. A miss issues the same queries as
  # before, later; a hit issues none of them.
  class PendingPreload
    def initialize(model, records, specs)
      @model = model
      @records = records
      @specs = specs
      @done = false
    end

    def run
      return nil if @done
      @done = true
      @model.preload_associations(@records, @specs)
      nil
    end
  end

  # A lazy, chainable query builder — the metaprogramming-free analog of
  # ActiveRecord::Relation. Lowered model code drives it: `scope`s become
  # class methods that take/return a Relation, associations return one,
  # and a query chain (`Model.where(...).order(...).limit(...)`) is a
  # sequence of Relation method calls that only touches the database at a
  # terminal (`to_a`/`each`/`first`/`count`/…).
  #
  # No `method_missing`, no `define_method`: every method is written out.
  # The model is held as a plain class-object value (`@model`) whose
  # `_table_sql` / `instantiate` class methods supply the per-model facts;
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
      @table = model._table_sql
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
      @skip_preloading = false
      @records = nil
      @scope_attributes = {}
      @where_columns = {}
      @hash_columns = []
      @from = nil
      @ctes = []
      @shared_lists = false
    end

    # Rails' `Relation#spawn`: a new relation that shares this one's
    # query state but not its accumulator arrays. Chain methods still
    # mutate in place; scopes call `spawn` on entry so a fork like
    # campfire's sidebar (`visible.with_direct_rooms` beside
    # `visible.with_ordered_room.without_direct_rooms`) does not let
    # one branch's joins/orders/wheres pollute the other.
    #
    # Copy-on-write: `clone` already shares the list objects. Mark both
    # sides shared and copy the lists on the first mutating chain method
    # instead of copying eight arrays on every scope entry. Accumulators
    # are copied element-wise (no `Array#dup` — keep the element type
    # the typer already knows); `@records` is shared until a chain
    # method clears it, matching Rails' loaded-spawn contract.
    def spawn
      @shared_lists = true
      clone
    end

    def own_lists
      if @shared_lists
        @shared_lists = false
        take_query_lists(
          @wheres,
          @joins,
          @orders,
          @groups,
          @havings,
          @ctes,
          @includes,
          @scope_attributes
        )
      end
      self
    end

    # List accumulators go through copy_* so inference types the
    # parameters. Scalars come from `clone` (shallow ivar copy).
    def take_query_lists(wheres, joins, orders, groups, havings, ctes, includes, attrs)
      @wheres = copy_string_list(wheres)
      @joins = copy_string_list(joins)
      @orders = copy_string_list(orders)
      @groups = copy_string_list(groups)
      @havings = copy_string_list(havings)
      @ctes = copy_string_list(ctes)
      @includes = copy_symbol_list(includes)
      @scope_attributes = copy_scope_attributes(attrs)
      self
    end

    def copy_string_list(xs)
      out = []
      xs.each { |x| out << x }
      out
    end

    def copy_symbol_list(xs)
      out = []
      xs.each { |x| out << x }
      out
    end

    def copy_scope_attributes(attrs)
      attrs
    end

    # ---- chain methods (return self) --------------------------------

    # `with_recursive(parents: [base, step])` — a recursive common table
    # expression, each named part the UNION ALL of its relations (Rails
    # 7.1). Rendered ahead of the SELECT; `from("parents")` then reads
    # from it. lobsters walks a comment's ancestors this way on the reply
    # page (`Comment#parents`).
    def with_recursive(ctes)
      own_lists
      @records = nil
      ctes.each do |name, parts|
        # Build the UNION list with pushes rather than `map.join`: the
        # body typer's Array#map still returns Untyped, so the join
        # terminal would stay Ty::Var under RBS-seeded typing.
        sql_parts = []
        parts.each { |p| sql_parts << p.to_sql }
        @ctes << "#{name} AS (#{sql_parts.join(" UNION ALL ")})"
      end
      self
    end

    # `from("parents")` — the FROM source, in place of this model's own
    # table. The String form only: Rails also takes a relation (a
    # subquery) there, which no corpus call writes.
    def from(source)
      @records = nil
      @from = source
      self
    end

    # `where(hash)` / `where("raw sql")` / `where("a = ? AND b = ?", x, y)`.
    def where(condition = nil, *args)
      add_condition(condition, args, false)
      self
    end

    # An association's scope: `where(fk => owner.id)` that ALSO presets
    # what a `create` through this relation writes.
    #
    # Rails derives both from the same place — a relation's equality
    # conditions filter reads (`where_values_hash`) and seed writes
    # (`scope_for_create`), which is why `user.sessions.create!(…)`
    # comes back with `user_id` set without anybody naming it. Only the
    # compiler's association seed calls this, so the ordinary `where`
    # stays a pure filter and pays nothing for the extra bookkeeping.
    def where_scope(condition)
      @scope_attributes = condition
      add_condition(condition, [], false)
      self
    end

    # Rails' `scope_for_create`: the attributes a record built through
    # this relation starts with. Empty for a relation nobody scoped.
    def scope_attributes
      @scope_attributes
    end

    # `where.not(...)` is lowered to `not(...)` on the relation: negate the
    # condition back onto this relation.
    def not(condition = nil, *args)
      add_condition(condition, args, true)
      self
    end

    # Rails' `excluding(…)` — everything in this relation except what is
    # named. Rails accepts records, arrays of records, and relations, and
    # all three already have a spelling here: `column_predicate` reads a
    # `Base` as its id, an `Array` as an IN list, and a `Relation` as an
    # IN subquery. So this is a negated primary-key condition and nothing
    # more — the polymorphism it looks like it needs is the polymorphism
    # `where` already carries.
    #
    # The one-argument case unwraps rather than passing the splat array
    # through: `excluding(some_relation)` has to reach the SUBQUERY
    # branch, and wrapped in an array it would reach the IN-list branch
    # and escape a Relation object into the SQL text.
    def excluding(*records)
      val = records.length == 1 ? records[0] : records
      own_lists
      @records = nil
      pred = column_predicate(@model.primary_key.to_s, val)
      @wheres << "NOT (#{pred})" unless pred.empty?
      self
    end

    # `without` — Rails' own alias for `excluding`, on both Relation and
    # Enumerable. campfire's sidebar splits its memberships in two and
    # writes `all_memberships.without(@direct_memberships)` for the
    # remainder.
    def without(*records)
      excluding(*records)
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

    # Answers whether it actually PUSHED a clause — a nil or empty
    # condition pushes none. `find_by` is the caller that needs the
    # answer: it borrows a predicate and gives it back, so it has to
    # know whether there is one to pop. The chain methods ignore it.
    def add_condition(condition, args, negate)
      own_lists
      @records = nil
      return false if condition.nil?
      sql = if condition.is_a?(Hash)
        hash_conditions(condition)
      else
        substitute_binds(condition.to_s, args)
      end
      return false if sql == ""
      clause = negate ? "NOT (#{sql})" : "(#{sql})"
      @wheres << clause
      remember_where_columns(clause, @hash_columns) if condition.is_a?(Hash)
      true
    end

    # Which columns a hash-built WHERE clause constrains, keyed by the
    # clause text, so `unscope(where:)` can take it back out. The text
    # is a pure function of the condition, so the table is a set of
    # facts every clone may share. A nested `table => { col => v }`
    # hash is recorded as `"table."`, which no bare `unscope(where: :col)`
    # target matches.
    def remember_where_columns(clause, cols)
      @where_columns[clause] = cols
      nil
    end

    # Rails' `unscope(where: :col)` / `unscope(where: [:a, :b])`: drop
    # every hash-built condition on those columns, keep the rest. Only
    # the `where:` form exists here, and a clause built from one hash
    # naming several columns cannot be split, so that case raises
    # instead of dropping the other columns' conditions. Raw-SQL
    # clauses are never touched, as in Rails.
    def unscope(spec)
      own_lists
      @records = nil
      targets = unscope_targets(spec)
      kept = []
      @wheres.each do |clause|
        cols = @where_columns[clause] || []
        hit = false
        cols.each { |c| hit = true if targets.include?(c) }
        if hit && cols.length > 1
          raise ArgumentError, "unscope(where:) cannot split the multi-column condition #{clause}"
        end
        kept << clause unless hit
      end
      @wheres = kept
      self
    end

    def unscope_targets(spec)
      spec.keys.each do |kind|
        raise ArgumentError, "unscope supports only where:, got #{kind}" unless kind == :where
      end
      targets = []
      names = spec[:where]
      if names.is_a?(Symbol)
        targets << names.to_s
      elsif names.is_a?(Array)
        i = 0
        while i < names.length
          targets << names[i].to_s
          i += 1
        end
      end
      targets
    end

    # `order` on a LOADED relation sorts the loaded records in memory
    # rather than dropping them and re-querying. This is the shape a
    # preloaded association takes through a scope: campfire's message
    # partial renders `message.boosts.ordered` for every message on the
    # page, and with `includes(boosts: :booster)` already loaded those
    # boosts, re-querying them was one round trip per message -- the room
    # page's last N+1. (Rails re-queries here too, and hides it behind
    # its fragment cache; a per-request renderer cannot.)
    #
    # Only a term that names a bare column, optionally with a table
    # prefix and ASC/DESC, sorts in memory; anything else (an expression,
    # a function, RANDOM()) drops the memo and the SQL path answers. The
    # sort is STABLE and applies the terms last-to-first, so ties under
    # the first term keep the loaded order -- which is the table's row
    # order, the same order SQLite hands back for tied keys -- and a
    # page sorted here matches one sorted by the database byte for byte.
    def order(*parts)
      own_lists
      terms = parts.map { |p| order_term(p) }
      terms.each { |t| @orders << t }
      loaded = @records
      return self if loaded.nil?
      # An explicit copy rather than `dup`: built by pushes, the copy has
      # the records array's own type where a `dup` of a nilable read does
      # not, and it is assigned back into `@records` below. An index loop
      # rather than `each`, whose block parameter the typer cannot derive
      # from a receiver it only knows as nilable.
      sorted = []
      i = 0
      while i < loaded.length
        sorted << loaded[i]
        i += 1
      end
      if sort_in_place!(sorted, terms)
        @records = sorted
      else
        @records = nil
      end
      self
    end

    # `order(Arel.sql("…"))` — a caller-authored ordering fragment, kept
    # as written (Rails takes an `Arel.sql` literal past its column-name
    # check; campfire's `reorder(Arel.sql("+messages.created_at"))`
    # keeps SQLite off an index). `lower::arel_sql_order` renames the
    # call; the bare String `order` still validates. A loaded relation
    # reads again rather than guess at a fragment's sort.
    def order_sql(fragment)
      own_lists
      @orders << fragment
      @records = nil
      self
    end

    def reorder_sql(fragment)
      @orders = []
      order_sql(fragment)
    end

    # `reorder(*parts)` — Rails' "replace the ordering": drop every term
    # gathered so far, then order by these. Same in-memory resort as
    # `order` when the records are already loaded.
    def reorder(*parts)
      @orders = []
      order(*parts)
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
      own_lists
      terms = parts.map { |p| order_term(p) }
      terms.each { |t| @orders << t }
      loaded = @records
      return self if loaded.nil?
      # An explicit copy rather than `dup`: built by pushes, the copy has
      # the records array's own type where a `dup` of a nilable read does
      # not, and it is assigned back into `@records` below. An index loop
      # rather than `each`, whose block parameter the typer cannot derive
      # from a receiver it only knows as nilable.
      sorted = []
      i = 0
      while i < loaded.length
        sorted << loaded[i]
        i += 1
      end
      if sort_in_place!(sorted, terms)
        @records = sorted
      else
        @records = nil
      end
      self
    end

    # Sort `sorted` (a copy of the loaded records) by `terms` in place;
    # true when every term was a bare column, false when one was not --
    # the caller then drops the memo and the database orders instead.
    # Stable insertion sort: the memo is a page, not a table, and
    # stability is the property that keeps tied rows in database order.
    def sort_in_place!(sorted, terms)
      i = terms.length - 1
      while i >= 0
        col = order_column(terms[i])
        return false if col.nil?
        desc = order_descending?(terms[i])
        # Keys once per row, not once per comparison: `attributes` builds
        # a Hash on every call.
        keys = sorted.map { |r| order_key_of(r, col) }
        n = sorted.length
        j = 1
        while j < n
          moving = sorted[j]
          key = keys[j]
          k = j - 1
          while k >= 0
            cmp = compare_order_keys(keys[k], key)
            return false if cmp.nil?
            cmp = -cmp if desc
            break if cmp <= 0
            sorted[k + 1] = sorted[k]
            keys[k + 1] = keys[k]
            k -= 1
          end
          sorted[k + 1] = moving
          keys[k + 1] = key
          j += 1
        end
        i -= 1
      end
      true
    end

    # One row's value under `col`, as the database would order it. The
    # synthesized `attributes` hash carries every column BUT the primary
    # key, and datetimes as their raw stored text (which orders as the
    # database orders them); `id` is read off the record itself.
    def order_key_of(record, col)
      return record.id if col == "id"
      record.attributes[col]
    end

    # The bare column an order term names -- `"created_at"` and
    # `"boosts.created_at DESC"` both answer `"created_at"` -- or nil for
    # anything that is not one column with an optional ASC/DESC.
    def order_column(term)
      words = term.strip.split(/\s+/)
      return nil if words.length == 0 || words.length > 2
      col = words[0]
      dot = col.rindex(".")
      col = col[(dot + 1)..] unless dot.nil?
      return nil unless col.match?(/\A[A-Za-z_][A-Za-z0-9_]*\z/)
      if words.length == 2
        dir = words[1].upcase
        return nil unless dir == "ASC" || dir == "DESC"
      end
      col
    end

    # Whether an order term (already accepted by `order_column`) says DESC.
    def order_descending?(term)
      words = term.strip.split(/\s+/)
      words.length == 2 && words[1].upcase == "DESC"
    end

    # Flip ASC/DESC so `last_n` can LIMIT the tail in SQL. A term with
    # no direction is ASC (SQLite and Rails). `order_term` joins a Hash
    # into one comma-separated string (`"a ASC, b DESC"`), and a raw
    # `"created_at DESC, id DESC"` is stored as one `@orders` entry, so
    # each comma-separated fragment is reversed on its own.
    def reverse_order_term(term)
      t = term.strip
      out = ""
      start = 0
      i = 0
      n = t.length
      while i <= n
        comma = false
        comma = true if i < n && t[i] == ","
        if i == n || comma
          part = t[start, i - start].to_s.strip
          if part.length > 0
            out = "#{out}, " if out.length > 0
            out = "#{out}#{reverse_one_order_term(part)}"
          end
          start = i + 1
        end
        i += 1
      end
      out
    end

    def reverse_one_order_term(term)
      t = term.strip
      upper = t.upcase
      if upper.end_with?(" DESC")
        "#{t[0, t.length - 5]} ASC"
      elsif upper.end_with?(" ASC")
        "#{t[0, t.length - 4]} DESC"
      else
        "#{t} DESC"
      end
    end

    # SQLite's ordering of two attribute values: NULL sorts first, then
    # like compares with like. nil for a pair this cannot order, which
    # sends the caller back to the database.
    def compare_order_keys(a, b)
      return 0 if a.nil? && b.nil?
      return -1 if a.nil?
      return 1 if b.nil?
      a <=> b
    end

    # The value is a request param as often as a literal
    # (`limit(params[:per_page])`), and it is spliced into the SQL, so
    # both pass through Rails' own casts here: `limit` is
    # `sanitize_limit` (`Integer()`, which raises on anything that is
    # not an integer) and `offset` is `build_arel`'s `to_i`. nil clears
    # either, as in Rails.
    def limit(n)
      @records = nil
      # Split rather than `n.nil? ? nil : sql_limit(n)`: Spinel cannot
      # unify nil with Integer in a conditional expression.
      if n.nil?
        @limit = nil
      else
        @limit = sql_limit(n)
      end
      self
    end

    def offset(n)
      @records = nil
      if n.nil?
        @offset = nil
      else
        @offset = n.to_i
      end
      self
    end

    # ---- page / per / paginate: LIMIT / OFFSET arithmetic ------------
    #
    # The catalog types `page`, `per`, and `paginate` as builders and
    # the readers below as terminals. `count` already leaves LIMIT and
    # OFFSET out of its SQL (`count_sql`), which is the unpaginated
    # total `total_count` needs. Not modeled: `padding`, `without_count`,
    # `max_per_page` / `max_pages`, and wrapping a loaded Array.
    #
    # The readers go through locals rather than doing arithmetic on the
    # ivars: a runtime ivar reads as `T | Nil` (see `ActionController::
    # Page`).

    # `page(n)`: page `n` at the app's default page size. A nil, blank,
    # non-numeric or non-positive `n` is page 1 (`to_i`).
    def page(num = nil)
      per_page = Rails.application.default_per_page
      n = num.to_s.to_i
      n = 1 if n < 1
      limit(per_page)
      offset((n - 1) * per_page)
    end

    # Same builder as `page` under the `paginate` spelling. A
    # positional page number, or `page:` / `per_page:` keywords.
    # `per(nil)` is a no-op (`per` only applies a numeric string).
    def paginate(num = nil, page: nil, per_page: nil)
      self.page(page || num).per(per_page)
    end

    # `per(n)`: the same page at `n` rows. A nil, blank or negative `n`
    # (the `/^\d/` test) leaves the relation as it is, so
    # `per(params[:per])` without the parameter keeps the default size;
    # `per(0)` is `limit(0)`.
    def per(num)
      text = num.to_s
      return self unless text.match?(/\A\d/)
      n = text.to_i
      return limit(0) if n == 0
      page_now = current_page
      limit(n)
      offset((page_now - 1) * n)
    end

    def limit_value
      @limit
    end

    def offset_value
      @offset
    end

    # 1 for a relation that was never paged (divide-by-nil limit);
    # `per(0)` raises ZeroDivisionError.
    def current_page
      per_page = @limit
      return 1 if per_page.nil?
      raise ZeroDivisionError, "Current page was incalculable. Perhaps you called .per(0)?" if per_page == 0
      skipped = @offset
      skipped = 0 if skipped.nil? || skipped < 0
      skipped / per_page + 1
    end

    def total_count
      count
    end

    # Rounded up; 0 for an empty relation, which makes page 1 of
    # nothing out of range rather than the last page. A relation that
    # was never paged is one page.
    def total_pages
      per_page = @limit
      return 1 if per_page.nil?
      raise ZeroDivisionError, "Total pages was incalculable. Perhaps you called .per(0)?" if per_page == 0
      (total_count + per_page - 1) / per_page
    end

    def first_page?
      current_page == 1
    end

    # Loaded non-empty short page cannot have a successor — skip COUNT.
    # Empty pages are ambiguous (page 1 of nothing vs. out of range).
    def last_page?
      per_page = @limit
      return true if per_page.nil?
      raise ZeroDivisionError, "Total pages was incalculable. Perhaps you called .per(0)?" if per_page == 0
      r = @records
      return true if !r.nil? && r.length > 0 && r.length < per_page
      current_page == total_pages
    end

    def out_of_range?
      current_page > total_pages
    end

    def next_page
      return nil if last_page? || out_of_range?
      current_page + 1
    end

    def prev_page
      return nil if first_page? || out_of_range?
      current_page - 1
    end

    def group(*parts)
      own_lists
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
      own_lists
      @records = nil
      @havings << substitute_binds(condition.to_s, args)
      self
    end

    # `joins("INNER JOIN memberships ON …")` — a raw SQL fragment, which
    # is what reaches this runtime. The ASSOCIATION form
    # (`joins(:users)`) is resolved at transpile time by
    # `lower::scope_chain`, which owns the only table that knows a
    # join's ON clause; a Symbol arriving here means that lowering
    # DECLINED, and appending the bare name produces `FROM rooms users`
    # — which SQLite reads as an alias, so the query runs and answers
    # the wrong rows.
    #
    # `AssocRegistry`'s own doc says an unresolvable shape is "left
    # untouched (visible at runtime rather than silently mis-joined)".
    # It was not visible: this method made it silent. Raising is what
    # that sentence always meant, and it names the association so the
    # ledger line and the failure agree.
    #
    # Found via `Rooms::Direct.all.joins(:users)` — an STI subclass is
    # not in the registry, so nothing could resolve it.
    # An identical join is added ONCE, as Rails' `joins_values` are
    # uniq'd: two scopes that each `joins(:story)` — lobsters'
    # `on_stories_not_authored_by.above_average` — render one INNER JOIN,
    # where appending both made SQLite reject every column of the joined
    # table as ambiguous.
    def joins(spec)
      own_lists
      @records = nil
      frag = join_fragment(spec)
      @joins << frag unless @joins.include?(frag)
      self
    end

    def left_outer_joins(spec)
      own_lists
      @records = nil
      frag = join_fragment(spec)
      @joins << frag unless @joins.include?(frag)
      self
    end

    def join_fragment(spec)
      return spec if spec.is_a?(String)
      raise("joins(#{spec}): an association join is resolved at transpile " \
            "time and this one was not — the receiver's class has no entry " \
            "in the association registry (an STI subclass, a habtm, or an " \
            "unresolvable `through`). Appending the name raw would answer " \
            "the wrong rows rather than fail.")
    end

    # `left_joins` — Rails alias for `left_outer_joins`.
    def left_joins(spec)
      left_outer_joins(spec)
    end

    # `select(:id, :username, "raw AS x")` — the PROJECTION, and the
    # only thing this name means here. Symbols qualify against this
    # relation's table (as Rails renders them); raw strings ride
    # verbatim.
    #
    # WITH A BLOCK Rails means a different method sharing the name:
    # Enumerable's filter over the loaded records, answering an Array.
    # That form lives on `filter` below, and `relation_select_block`
    # lowers every relation-typed `select { … }` call site onto it — so
    # this method answers a Relation and nothing else.
    #
    # It is worth saying why the fork is a lowering rather than a
    # `specs.empty?` branch here, which is what it used to be. A method
    # answering `Relation | Array` types its whole receiver chain POLY,
    # and on the strict targets poly is not merely the slow path: it is
    # a DIFFERENT dispatch path. spinel's does not apply the
    # braceless-keyword-args → trailing-positional-Hash conversion, so
    # `Story.select(:id).where(merged_story_id: id)` bound `where`'s
    # optional `condition` to its `nil` default and dropped the filter
    # on the floor — lobsters' `/s/:story_id` rendered the comments of
    # every merged story instead of its own, with no warning anywhere.
    #
    # Zero specs therefore means the lowering did not fire, which is a
    # compiler bug and not a shape to guess at: the old branch's failure
    # mode (`@select_sql` an EMPTY string, so the next hop emitted
    # `SELECT  FROM memberships …`) was silent until something forced
    # the query. Raise instead.
    def select(*specs)
      raise ArgumentError, "select: no columns (a block form reached the projection)" if specs.empty?
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
      own_lists
      @records = nil
      names.each { |n| @includes << n }
      self
    end

    def preload(*names)
      own_lists
      @records = nil
      names.each { |n| @includes << n }
      self
    end

    # `skip_preloading!` — load the rows without running the recorded
    # `includes`/`preload` specs. The specs stay on the relation, so a
    # caller can apply them later, to just the records it needs, with
    # `preload_associations`. campfire's message pages load this way and
    # preload only the messages its fragment cache missed.
    def skip_preloading!
      @records = nil
      @skip_preloading = true
      self
    end

    # `preload_associations(records)` — run this relation's recorded
    # preload specs against `records`, loaded here or anywhere else, the
    # batched `IN` loads `load_records` would have run on its own rows.
    def preload_associations(records)
      @model.preload_associations(records, @includes) if @includes.length > 0
      records
    end

    def eager_load(*names)
      own_lists
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
      own_lists
      @records = nil
      other.where_clauses.each { |w| @wheres << w }
      other.where_columns.each { |clause, cols| @where_columns[clause] = cols }
      self
    end

    def where_clauses
      @wheres
    end

    def where_columns
      @where_columns
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
      own_lists
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

    # Rails' `load_async` schedules the query on a background pool and
    # answers the relation; the first read waits for the rows. Loading
    # now is the same observable answer — this runtime has no async
    # executor to overlap the query with, and lobsters' story page
    # (`@story.comments…load_async`) reads the records a few lines on.
    def load_async
      load
    end

    # An eager load's records, handed to the relation that would have
    # gone and fetched them. A `has_many :through` reader answers a
    # Relation (the join lives on the intermediate table, so there is no
    # direct-fk query to materialize), while `includes(:tags)` preloads
    # through `_preload_tags` into the owner's cache ivar — this is the
    # seam between the two: the reader builds its joined relation and
    # seeds the loaded-records memo with what the preload already
    # fetched, so no query runs and the declared return type stays
    # Relation on every path through the reader.
    #
    # `loaded` is a separate argument because the cache cannot answer
    # the question: every association ivar starts `[]` in `initialize`,
    # which is indistinguishable from an association that loaded and
    # found nothing. False is a plain no-op.
    #
    # Chaining on afterwards drops the seed like any other chain method
    # (`where`/`order`/… all clear `@records`), so a caller that
    # narrows a preloaded relation re-queries rather than filtering a
    # stale set — Rails' behavior for a loaded relation, and the reason
    # the join conditions stay on the relation instead of this method
    # answering the bare Array.
    def preloaded(records, loaded)
      @records = records if loaded
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
      records = load_records
      @records = records
      records.dup
    end

    # The rows, hydrated. With no explicit `select`, the projection is
    # the model's own column list and the model hydrates typed records
    # straight from the statement (`_hydrate_all`) — the positions are
    # fixed at compile time, so no String-keyed Hash per row. An
    # explicit `select` can project anything, and keeps the Hash path.
    def load_records
      records = if @select_sql.nil?
        @model._hydrate_all(select_sql_with(@model._columns_sql))
      else
        rows = ActiveRecord.adapter.select_rows(to_sql)
        rows.map { |row| @model.instantiate(row) }
      end
      if @includes.length > 0 && !@skip_preloading && records.length > 0
        pending = ActiveRecord::PendingPreload.new(@model, records, @includes.dup)
        records.each { |record| record._pend_preload(pending) }
      end
      records
    end

    # Implicit array conversion — Rails delegates `to_ary` to the
    # loaded records, which is what lets `[story, relation].flatten`
    # splice the relation's records into the surrounding Array
    # (Array#flatten recurses into elements that respond to to_ary).
    # `relation.to_set` — Enumerable's, on the loaded records (campfire's
    # `Rooms::Direct.find_or_create_for(...).users.to_set`, comparing
    # direct-room members whatever their order).
    def to_set
      Set.new(to_a)
    end

    def to_ary
      to_a
    end

    # `relation == array` — Rails compares the LOADED RECORDS, and this
    # covers BOTH orders. Ruby's `Array#==` hands the comparison over to
    # the other operand whenever that operand answers `to_ary` (which
    # the method above does), so `[ message ] == Message.search("eel")`
    # arrives here as `relation == [ message ]`. Without it that
    # expression fell through to `Object#==` — reference identity
    # between an Array and a Relation, which is false for every input,
    # so `assert_equal <array>, <relation>` was an assertion no emitted
    # tree could pass. campfire's `message_searchable_test` is three
    # tests written entirely in that shape.
    #
    # Relation-to-relation compares records too, rather than Rails'
    # `to_sql` equality: two relations that differ only in how they were
    # BUILT are the same result set here, and the result set is what a
    # caller is asking about.
    #
    # PRIMARY KEYS, not `==` on the elements, and that is the whole
    # point rather than a shortcut. Rails answers this through
    # `ActiveRecord::Core#==`, where two objects are the same record
    # when they share a class and a saved id — a row read twice is two
    # objects. This runtime has no such `Base#==`: an operator
    # DEFINITION in `base.rb` reaches every strict target, and no
    # emitter renames one to its host's spelling (measured — python
    # wrote `def ==(self, other)` verbatim and every emitted tree
    # stopped at a SyntaxError). A relation's records are all one model
    # by construction, so comparing ids here is that rule with the class
    # half already decided. Record-to-record `==` remains identity, and
    # closing it means teaching each emitter the rename.
    def ==(other)
      mine = to_a
      theirs = other.is_a?(ActiveRecord::Relation) ? other.to_a : other
      return false if !theirs.is_a?(Array)
      return false if mine.length != theirs.length
      ids_of(mine) == ids_of(theirs)
    end

    # `filter { |r| … }` — Enumerable's filter over the loaded records.
    # `select { |r| … }` is Rails' other spelling of this method and
    # arrives here renamed (`relation_select_block`), which leaves the
    # projection `select(*specs)` above monomorphic.
    def filter
      out = []
      loaded_records.each { |x| out << x if yield x }
      out
    end

    # `relation + array` — Rails materializes and concatenates
    # (`to_a + other`), yielding a plain Array. The set operations
    # (`&`, `|`, `-`) are delegated to the loaded records the same
    # way (ActiveRecord::Delegation's array-method delegation);
    # lobsters intersects `story.tags & filtered_tags`.
    #
    # The other operand may be a Relation too — `filtered_tags` is
    # one — and Rails' Array operators take it through `to_ary`, so it
    # is loaded here the way `==` below loads it; handed over as is, a
    # Relation reached Array#& and raised TypeError. Membership is by
    # PRIMARY KEY for the reason `==` gives: Rails' `Array#&` asks the
    # records' `eql?`/`hash`, which ActiveRecord::Core answers by class
    # and id, and this runtime has no such `Base#==` — a row loaded by
    # each side is two objects, and identity would intersect them to
    # nothing. Like Array's, `&` and `|` drop repeats; `-` does not.
    def +(other)
      to_a + set_operand(other)
    end

    def &(other)
      id_filter(to_a, operand_ids(other), true)
    end

    def |(other)
      to_a + id_filter(set_operand(other), ids_of(to_a), false)
    end

    def -(other)
      id_filter(to_a, operand_ids(other), false)
    end

    # The records whose id is (`keep`) or is not in `ids`.
    def id_filter(records, ids, keep)
      records.select { |r| ids.include?(r.id) == keep }
    end

    def set_operand(other)
      other.is_a?(ActiveRecord::Relation) ? other.to_a : other
    end

    def operand_ids(other)
      ids_of(set_operand(other))
    end

    def ids_of(records)
      records.map { |r| r.id }
    end

    # Loaded: scan the cache. Unloaded: exists?(id), not ids (ORDER BY).
    def include?(record)
      return false if record.nil?
      rid = record.id
      return false if rid.nil?
      key = @model._cast_primary_key(rid)
      return false if key.nil?
      loaded = @records
      unless loaded.nil?
        return loaded.any? { |x| x.id == key }
      end
      exists?(key)
    end

    # Memoized rows, not a dup. each/find_each return self.
    def loaded_records
      records = @records
      if records.nil?
        records = load_records
        @records = records
      end
      records
    end

    def each
      records = loaded_records
      i = 0
      n = records.length
      while i < n
        yield records[i]
        i += 1
      end
      self
    end

    # `index_by { |r| key }` — the records as a Hash keyed by the
    # block's value, last write winning on duplicates (Rails'
    # contract; lobsters keys tag filters by id).
    def index_by
      h = {}
      loaded_records.each { |x| h[yield x] = x }
      h
    end

    # `find_each` — Rails batches; corpus sizes make one load the same
    # answer. Loop duplicated from `each`: a nested `{ |x| yield x }`
    # left `x` as TyVar under Bar A.
    def find_each
      records = loaded_records
      i = 0
      n = records.length
      while i < n
        yield records[i]
        i += 1
      end
      self
    end

    # `find_in_batches` — Rails yields successive Arrays of rows.
    # Corpus sizes make one load the same answer as find_each; yield
    # the whole page as a single batch. Campfire's unread fanout /
    # push paths call this on memberships.
    def find_in_batches
      records = loaded_records
      yield records
      self
    end

    # Not split into `of:`-row relations: one batch is the same answer at corpus sizes (as `find_in_batches`), so the block and the enumerator are this relation.
    def in_batches(of: 1000, order: nil)
      yield self if block_given?
      self
    end

    # Via loaded_records (not to_a): no shallow Array copy of the
    # memoized rows. to_a keeps its Rails dup contract for callers that
    # mutate the returned array.
    def map
      loaded_records.map { |x| yield x }
    end

    # Enumerable's `to_h { |rec| [k, v] }` over the materialized rows
    # (campfire's push pool test keys each subscription's badge by its
    # endpoint this way).
    def to_h
      loaded_records.to_h { |x| yield x }
    end

    # `collect` is Enumerable's second name for `map`, and Rails
    # relations answer it because they delegate the whole of Enumerable
    # to `to_a`. campfire's membership extension reaches it
    # (`Array(users).collect { … }` where `users` is a relation).
    # Duplicated rather than aliased: strict targets want a real
    # definition, not an alias, and a body forwarding to `map` would
    # have to forward the block too.
    def collect
      loaded_records.map { |x| yield x }
    end

    # `group_by { |rec| key }` — Enumerable's grouping over the
    # materialized rows (lobsters threads comments with
    # `@comments.group_by(&:parent_comment_id)`). fetch-then-insert
    # rather than `Hash.new { [] }` (no default-proc portability) or
    # `[]=`-chaining on a maybe-missing key.
    def group_by
      out = {}
      loaded_records.each do |rec|
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

    # `partition { |r| … }` — Enumerable's two-way split over the
    # materialized rows, `[matching, rest]`. campfire's account page
    # writes `@administrators, @members = users.partition(&:administrator?)`
    # straight off a `User.where(...)`.
    def partition
      loaded_records.partition { |x| yield x }
    end

    # `detect { |r| … }` — Enumerable's first match, nil when none.
    # (`find` is NOT this: on a Relation that is Rails' find-by-id.)
    def detect
      to_a.detect { |x| yield x }
    end

    # `sort_by { |r| key }` — Enumerable's sort over the materialized
    # rows. Distinct from `order`, which is SQL: this one sorts by a
    # value the block computes in Ruby, which is why campfire reaches
    # for it to sort direct rooms by their room's `updated_at`.
    def sort_by
      to_a.sort_by { |x| yield x }
    end

    # `inject(initial) { |acc, x| ... }` — the accumulator form the
    # corpus uses (vote-hash batchers). The no-initial and Symbol forms
    # aren't modeled; callers pass an explicit seed.
    def inject(initial)
      acc = initial
      to_a.each { |x| acc = yield(acc, x) }
      acc
    end

    # `each_with_object(memo) { |r, memo| … }` — Enumerable's fold that
    # threads one mutable memo and answers it. lobsters builds its vote
    # lookup tables this way straight off a query
    # (`Vote.where(…).select(…).each_with_object({}) { |v, memo| … }`,
    # on every comment listing).
    def each_with_object(memo)
      to_a.each { |x| yield(x, memo) }
      memo
    end

    def first
      prior = @limit
      @limit = 1
      rows = to_a
      @limit = prior
      # The one-row load must NOT stay memoized: `@records` is the
      # loaded-relation cache a later `each`/`map` reads, and a cache
      # holding the single row `first` asked for would answer those with
      # one row out of many. Same terminal rule as `pluck`.
      @records = nil
      rows.length == 0 ? nil : rows[0]
    end

    # `take` — a row with no ordering imposed. Rails leaves the order
    # to the database; SQLite hands back the lowest rowid, which is the
    # row `first` orders to, so one query shape serves both.
    def take
      first
    end

    # `first!` — like `first`, but raises `RecordNotFound` (→ 404 in the
    # dispatch layer) instead of returning nil when the relation is empty.
    def first!
      record = first
      raise RecordNotFound.new("Couldn't find #{@model.name}", @model.name, @model.primary_key) if record.nil?
      record
    end

    def last
      rows = last_n(1)
      rows.length == 0 ? nil : rows[0]
    end

    # Rails' `first(n)` / `last(n)` — the COUNTED forms, which answer an
    # Array of up to n records where the bare forms answer one record or
    # nil. Split into their own names rather than overloaded onto
    # `first`/`last` with an optional arg: the return type differs by
    # arity, which a strict target cannot express on one method (see the
    # monomorphize-polymorphic-APIs rule). `scope_chain` renames the call
    # site once it has PROVEN the receiver is a relation, so an Array
    # receiver keeps `Array#first(n)` — lobsters' `split.first(words * 2)`
    # must not be rewritten.
    #
    # Also a TERMINAL, so the borrowed `@limit` is restored — as
    # `first` and `pick` already do — and the one-page load is not
    # left memoized. Otherwise a relation that is paged and then
    # counted carries the page size into the count.
    def first_n(n)
      n = sql_limit(n)
      prior = @limit
      @limit = n
      rows = to_a
      @limit = prior
      @records = nil
      rows
    end

    # Last n in relation order (Rails does not reverse the page).
    # Unloaded and unwindowed: reverse ORDER BY, LIMIT n, reverse rows.
    # Loaded or already LIMIT/OFFSET: in-memory tail of that window.
    def last_n(n)
      n = sql_limit(n)
      loaded = @records
      unless loaded.nil?
        return loaded_tail(loaded, n)
      end
      unless @limit.nil? && @offset.nil?
        return to_a.last(n)
      end
      own_lists
      prior_limit = @limit
      prior_orders = []
      i = 0
      while i < @orders.length
        prior_orders << @orders[i]
        i += 1
      end
      if @orders.empty?
        @orders << "#{@table}.#{@model.primary_key} DESC"
      else
        reversed = []
        i = 0
        while i < @orders.length
          reversed << reverse_order_term(@orders[i])
          i += 1
        end
        @orders = reversed
      end
      @limit = n
      rows = to_a
      @limit = prior_limit
      @orders = prior_orders
      @records = nil
      out = []
      i = rows.length - 1
      while i >= 0
        out << rows[i]
        i -= 1
      end
      out
    end

    # The last n of an already-loaded page, still in relation order.
    def loaded_tail(loaded, n)
      start = loaded.length - n
      start = 0 if start < 0
      out = []
      i = start
      while i < loaded.length
        out << loaded[i]
        i += 1
      end
      out
    end

    # `sole`'s loaded-relation reading: the first two (or fewer) of an
    # already-loaded page, still in relation order — the loaded-cache
    # counterpart to `loaded_tail`, and every `loaded`-touching step of
    # it (the cap, the indexing loop) lives in THIS top-level helper
    # rather than inline in `sole` itself. `@records`'s own ivar type
    # carries an untyped/poly component, and `sole` also assigns the
    # unloaded branch's `to_a` Array to a `rows` local of its own;
    # splitting the loaded branch's work out to a call boundary, same
    # as `loaded_tail`, gives `loaded` here its own concrete `Array
    # [Base]` type from the narrowed (non-nil) argument rather than
    # carrying that poly residue into `sole`'s body, which is what the
    # ceiling tests on this runtime's type precision caught.
    def loaded_sole_rows(loaded)
      cap = loaded.length < 2 ? loaded.length : 2
      out = []
      i = 0
      while i < cap
        out << loaded[i]
        i += 1
      end
      out
    end

    # `sole`'s unloaded-probe size: `min(2, limit)`, nil meaning the
    # usual 2, clamped at 0 or above. A top-level helper rather than a
    # local computed inline in `sole` with an if/else (one arm the
    # literal `2`, the other derived from `@limit`): merging those two
    # origins into one local left it an unresolved type variable where
    # `sole` assigns it back to `@limit` — same `Integer? -> Integer`
    # call-boundary fix as `loaded_head`, above.
    def sole_probe_limit(limit)
      return 2 if limit.nil?
      return 0 if limit < 0
      return 2 if limit > 2
      limit
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

    # `minimum(:column)` / `maximum(:column)` — SQL extrema over the
    # current relation. The aggregate replaces the projection and ordering,
    # while the relation's filters, grouping and pagination remain in place.
    def minimum(expr)
      extreme(expr, "MIN")
    end

    def maximum(expr)
      extreme(expr, "MAX")
    end

    def extreme(expr, function)
      column = expr.to_s.to_sym
      raise ArgumentError, "unknown aggregate column: #{expr}" unless @model.schema_columns.include?(column)
      return grouped_extreme(column.to_s, function) if @groups.length > 0
      scoped = spawn
      projection = "#{function}(#{@table}.#{ActiveRecord.adapter.quote_column_name(column)}) AS value"
      projection = "#{@select_sql}, #{projection}" if @havings.length > 0 && !@select_sql.nil?
      scoped.select(projection)
      scoped.reorder
      rows = ActiveRecord.adapter.select_rows(scoped.to_sql)
      return nil if rows.length == 0
      value = rows[0]["value"]
      cast_schema_value(value, column)
    end
    private :extreme

    # Grouped calculations return a key-to-extreme Hash, as Rails does.
    # Aliasing each group expression keeps both scalar keys and composite
    # Array keys stable even when a group expression is qualified.
    def grouped_extreme(column, function)
      names = []
      selects = []
      @groups.each_with_index do |group, index|
        name = "__rh_group_#{index}"
        names << name
        selects << "#{group} AS #{name}"
      end
      selects << "#{function}(#{@table}.#{ActiveRecord.adapter.quote_column_name(column)}) AS value"
      scoped = spawn
      selects.unshift(@select_sql) if @havings.length > 0 && !@select_sql.nil?
      scoped.select(selects.join(", "))
      scoped.reorder
      rows = ActiveRecord.adapter.select_rows(scoped.to_sql)
      grouped = {}
      rows.each do |row|
        key_values = []
        names.each_with_index do |name, index|
          value = row[name]
          group = @groups[index]
          group_column = schema_group_column(group)
          key_values << (group_column.nil? ? value : cast_schema_value(value, group_column))
        end
        key = names.length == 1 ? key_values[0] : key_values
        grouped[key] = cast_schema_value(row["value"], column.to_sym)
      end
      grouped
    end
    private :grouped_extreme

    # Recover a model-column group key only from a bare or explicitly
    # model-qualified identifier. Arbitrary SQL and joined-table columns
    # remain untyped rather than borrowing a coincidentally named schema field.
    def schema_group_column(group)
      expression = group.strip
      table_name = @model.table_name.to_s
      quoted_table = "\"#{table_name.gsub("\"", "\"\"")}\""
      prefixes = ["#{@table}.", "#{table_name}.", "#{quoted_table}."]
      column = nil
      prefixes.each do |prefix|
        if expression.start_with?(prefix)
          column = expression[prefix.length..]
          break
        end
      end
      column = expression if column.nil?
      if column.start_with?("\"") && column.end_with?("\"")
        column = column[1...-1].gsub("\"\"", "\"")
      end
      return nil unless column.match?(/\A[A-Za-z_][A-Za-z0-9_]*\z/)
      candidate = column.to_sym
      @model.schema_columns.include?(candidate) ? candidate : nil
    end
    private :schema_group_column

    # Raw aggregate rows need the same schema-selected conversions as
    # hydrated model fields; otherwise SQLite returns temporal values as text.
    def cast_schema_value(value, column)
      return nil if value.nil?
      return ActiveSupport.cast_boolean(value) if @model.schema_boolean_columns.include?(column)
      return ActiveSupport.parse_db_time(value) if @model.schema_time_columns.include?(column)
      return cast_schema_date(value) if @model.schema_date_columns.include?(column)
      return value.to_f if @model.schema_decimal_columns.include?(column)
      value
    end
    private :cast_schema_value

    # Not `ActiveSupport.parse_db_date`: Spinel has it only with the Date package, which redefines this; a date-free app has no date column.
    def cast_schema_date(value)
      value
    end
    private :cast_schema_date

    # `group(:col).count` — Rails hands back a Hash of group-key =>
    # COUNT. The group_count lowering renames the grouped chain's
    # terminal to this method, so the scalar `count` keeps its
    # Integer return (no polymorphic count). Single group expression
    # (the corpus shape); Rails' multi-group array keys would need a
    # composite key here first. HAVING and DISTINCT ride the same SQL
    # as `to_sql` so the Hash only contains surviving groups, and a
    # distinct projection counts distinct values per group (#343).
    def group_count
      key = @groups.join(", ")
      # DISTINCT uses a subquery so a multi-column / aliased `select`
      # never lands inside SQLite's one-expression COUNT(DISTINCT …).
      # HAVING filters groups on the original grouped relation first
      # (source columns and aggregates), then distinct-count rows that
      # belong to survivors. Non-distinct keeps `@select_sql` so HAVING
      # can name selected aliases, matching `count_sql`.
      sql = if @distinct
        inner = append_join_where(
          "#{cte_prefix}SELECT DISTINCT #{distinct_count_columns}, #{key} AS k FROM #{from_source}"
        )
        if @havings.length > 0
          survivors = append_group_having(
            append_join_where("#{cte_prefix}SELECT #{key} AS __rh_k FROM #{from_source}")
          )
          clause = "#{key} IN (SELECT __rh_k FROM (#{survivors}) AS __rh_surv)"
          inner = if @wheres.length > 0
            "#{inner} AND #{clause}"
          else
            "#{inner} WHERE #{clause}"
          end
        end
        "SELECT k, COUNT(*) AS n FROM (#{inner}) AS __rh_gc GROUP BY k"
      else
        proj = if @select_sql.nil?
          "#{key} AS k, COUNT(*) AS n"
        else
          "#{@select_sql}, #{key} AS k, COUNT(*) AS n"
        end
        append_group_having(
          append_join_where("#{cte_prefix}SELECT #{proj} FROM #{from_source}")
        )
      end
      h = {}
      rows = ActiveRecord.adapter.select_rows(sql)
      rows.each { |row| h[row["k"]] = row["n"].to_i }
      h
    end

    # Loaded: cache length. Unloaded: exists? (SELECT 1 LIMIT 1).
    def empty?
      r = @records
      r.nil? ? !exists? : r.length == 0
    end

    def any?
      r = @records
      r.nil? ? exists? : r.length > 0
    end

    # Rails loads for blank?; empty? keeps the existence probe.
    # lower::blank folds typed sites; these cover untyped dispatch.
    def blank?
      empty?
    end

    def present?
      !empty?
    end

    def presence
      empty? ? nil : self
    end

    # Enumerable#none? without a block is empty? (uses the loaded cache).
    def none?
      empty?
    end

    # Exactly one row. Unloaded: SELECT 1 LIMIT 2. No block form.
    def one?
      r = @records
      return r.length == 1 unless r.nil?
      probe_existence(2) == 1
    end

    # More than one row. Unloaded: SELECT 1 LIMIT 2.
    def many?
      r = @records
      return r.length > 1 unless r.nil?
      probe_existence(2) > 1
    end

    # Block form of Enumerable#all? over the materialized rows (the
    # runtime `Base.where` fallback returns a Relation, and dynamic
    # call-sites treat it as the array Rails hands back).
    def all?
      ok = true
      loaded_records.each { |x| ok = false unless yield x }
      ok
    end

    # Hash conditions share where's predicate builder; scalar conditions
    # select the primary key. Unloaded: exists_sql (SELECT 1 LIMIT 1).
    def exists?(id = nil)
      return false if @limit == 0
      if id.nil?
        r = @records
        return r.length > 0 unless r.nil?
        return probe_existence(1) > 0
      end
      own_lists
      # Popped for the same reason `find` and `find_by` pop: a terminal
      # that answered a question must not narrow the relation it was
      # asked on.
      sql = if id.is_a?(Hash)
        hash_conditions(id)
      else
        "#{@table}.#{@model.primary_key} = #{ActiveRecord.adapter.escape_value(id)}"
      end
      @wheres << sql unless sql.empty?
      begin
        probe_existence(1) > 0
      ensure
        @wheres.pop unless sql.empty?
      end
    end

    # How many probe rows `exists_sql(n)` returns. Shared by `exists?`,
    # `one?`, and `many?` so cardinality questions never hydrate.
    def probe_existence(n)
      return 0 if @limit == 0
      ActiveRecord.adapter.select_rows(exists_sql(n)).length
    end

    # `count > n` without a COUNT(*): same FROM/JOIN/WHERE as
    # `count_sql` (LIMIT/OFFSET/ORDER ignored), then `LIMIT 1 OFFSET n`.
    # Does not mutate the relation — `offset(n).exists?` would, and a
    # loaded `exists?` would ignore that offset.
    def more_than?(n)
      return true if n < 0
      sql = "#{cte_prefix}SELECT 1 AS one FROM #{from_source}"
      sql = "#{sql} #{@joins.join(" ")}" if @joins.length > 0
      sql = "#{sql} WHERE #{@wheres.join(" AND ")}" if @wheres.length > 0
      sql = "#{sql} LIMIT 1 OFFSET #{n}"
      ActiveRecord.adapter.select_rows(sql).length > 0
    end

    def length
      loaded_records.length
    end

    # Rails' `Relation#size`: length when loaded; COUNT when unloaded
    # and unbounded. A LIMIT/OFFSET window must not answer the table
    # total — `limit(2).size` is at most 2 — so the limited path counts
    # a `SELECT 1` subquery rather than `count_sql` (which omits LIMIT
    # so page totals see the unpaginated result set).
    def size
      r = @records
      return r.length unless r.nil?
      return count if @limit.nil? && @offset.nil?
      prior_orders = @orders
      @orders = []
      # Same DISTINCT pitfall as exists_sql: `SELECT DISTINCT 1` collapses
      # every matching row into one, so `distinct.limit(5).size` would
      # answer 1. Project the primary key when distinct so the outer
      # COUNT sees separate rows under LIMIT.
      cols = @distinct ? "#{@table}.#{@model.primary_key}" : "1 AS one"
      inner = select_sql_with(cols)
      @orders = prior_orders
      sql = "SELECT COUNT(*) AS n FROM (#{inner}) AS __rh_size"
      rows = ActiveRecord.adapter.select_rows(sql)
      rows.length == 0 ? 0 : rows[0]["n"].to_i
    end

    # `delete_all` — bulk DELETE scoped by the accumulated WHEREs.
    # Rails contract: no callbacks, no per-row loads, returns the
    # affected-row count. ORDER/LIMIT don't apply to bulk ops.
    def delete_all
      sql = "DELETE FROM #{@table}#{scoped_write_where}"
      ActiveRecord.adapter.execute_ddl(sql)
      ActiveRecord.adapter.changes
    end

    # `destroy_all` — load the scoped records and destroy each one, so
    # `before_destroy` / `after_destroy` and any dependent-association
    # cleanup RUN. Deliberately not `delete_all` with a different name:
    # Rails draws exactly this line, and campfire depends on the
    # callback half (`Search#trim_recent_searches` prunes a user's
    # search history through it). The Array of destroyed records is
    # Rails' return value too.
    def destroy_all
      records = to_a
      records.each { |r| r.destroy }
      records
    end

    # `destroy_by(conditions)` — Rails' `where(conditions).destroy_all`,
    # and spelled as exactly that so the callback contract `destroy_all`
    # documents above carries over unchanged. campfire's
    # `Room#memberships.revoke_from` is the caller.
    def destroy_by(conditions)
      where(conditions).destroy_all
    end

    # `delete_by(conditions)` — `destroy_by`'s callback-skipping twin,
    # the same line Rails draws between `destroy_all` and `delete_all`.
    def delete_by(conditions)
      where(conditions).delete_all
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
      sql = "UPDATE #{@table} SET #{set_sql}#{scoped_write_where}"
      ActiveRecord.adapter.execute_ddl(sql)
      ActiveRecord.adapter.changes
    end

    # `touch_all(:col)` — Rails' bulk touch: `updated_at` (when the table
    # has one) and the named column set to the current time, by one
    # UPDATE scoped like `update_all`, no callbacks. lobsters marks its
    # inbox read this way after every inbox page (`@notifications
    # .where(read_at: nil).touch_all(:read_at)`). One column rather than
    # Rails' `*names`: that is every corpus call, and a splat of names
    # would reach the SQL untyped.
    def touch_all(name = nil)
      now = ActiveRecord.adapter.escape_value(ActiveSupport.db_now)
      parts = []
      parts.push("updated_at = #{now}") if @model.schema_columns.include?(:updated_at)
      parts.push("#{name} = #{now}") unless name.nil?
      return 0 if parts.empty?
      ActiveRecord.adapter.execute_ddl("UPDATE #{@table} SET #{parts.join(", ")}#{scoped_write_where}")
      ActiveRecord.adapter.changes
    end

    # The WHERE clause a bulk WRITE takes — the one place `delete_all`
    # and `update_all` differ from every read on this class.
    #
    # A read appends `@joins` to its FROM; SQL has no such place in a
    # DELETE or an UPDATE, and dropping the join while KEEPING the
    # conditions that name it emits a statement about a table that is
    # not in the query. campfire's `User#deactivate` runs
    # `memberships.without_direct_rooms.delete_all`, whose scope is
    # `joins(:room).where.not(room: { type: "Rooms::Direct" })`, and it
    # produced `DELETE FROM memberships WHERE … AND NOT (room.type =
    # 'Rooms::Direct')` — "no such column: room.type".
    #
    # Rails answers the same way: scope the write by a subquery on the
    # primary key, which is where the join CAN live. The `IN (SELECT …)`
    # form is what SQLite supports (it has no `DELETE … USING`), and it
    # is portable to every adapter this runtime might grow.
    #
    # Returns "" for the unscoped case so an unconditional `delete_all`
    # still emits a bare `DELETE FROM <table>` — Rails' truncate-shaped
    # statement, not a subquery over every row.
    def scoped_write_where
      return "" if @wheres.length == 0 && @joins.length == 0
      if @joins.length == 0
        return " WHERE #{@wheres.join(" AND ")}"
      end
      key = "#{@table}.#{@model.primary_key}"
      inner = "SELECT #{key} FROM #{@table} #{@joins.join(" ")}"
      inner = "#{inner} WHERE #{@wheres.join(" AND ")}" if @wheres.length > 0
      " WHERE #{@model.primary_key} IN (#{inner})"
    end

    # `pluck(:col)` — a single column projected to an Array of its raw
    # values (strings as stored; callers coerce).
    #
    # THE PROJECTION IS RESTORED, and that is not housekeeping. `pluck`
    # is a TERMINAL: Rails builds it a query of its own and leaves the
    # receiver alone, so the same relation can be plucked and then
    # loaded. Leaving `users.id AS v` behind meant the NEXT `to_a` on
    # that object hydrated whole records out of a one-column row —
    # every field blank, every id 0, no error anywhere. campfire's
    # `Rooms::Direct.find_or_create_for` does exactly that: `find_for`
    # plucks the user ids, then hands the SAME relation to `grant_to`,
    # which built memberships for user 0.
    def pluck(col)
      prior = @select_sql
      @select_sql = "#{@table}.#{col} AS v"
      rows = ActiveRecord.adapter.select_rows(to_sql)
      @select_sql = prior
      rows.map { |row| row["v"] }
    end

    # `pick(col)` — Rails' `limit(1).pluck(col).first`: the single value
    # from the first row, or nil when the relation matches nothing.
    # Lobsters reads every Keystore counter through it.
    def pick(col)
      prior = @limit
      @limit = 1
      rows = pluck(col)
      @limit = prior
      rows.length == 0 ? nil : rows[0]
    end

    # `ids` — primary keys, cast through the model's key type. Reads
    # `@model.primary_key` (not a hard-coded `id` column) and casts the
    # way `find` does, so uuid / string keys survive (#310).
    def ids
      prior = @select_sql
      key = @model.primary_key
      @select_sql = "#{@table}.#{key} AS v"
      rows = ActiveRecord.adapter.select_rows(to_sql)
      @select_sql = prior
      rows.map { |row| @model._cast_primary_key(row["v"]) }
    end

    # `find(id)` — the row with that primary key, RAISING
    # `RecordNotFound` when there is none. That raise is Rails' whole
    # distinction between `find` and the `find_by` below it, and it is
    # what turns a missing record into a 404 instead of a nil that
    # NoMethodErrors somewhere later. This answered nil until now, and
    # campfire's autocomplete is where that showed: `Current.user.rooms
    # .find(params[:room_id]).users` on a room the user is not a member
    # of read "undefined method 'users' for nil" — the test asserting
    # `assert_raises ActiveRecord::RecordNotFound` on exactly that
    # request.
    #
    # The id predicate is POPPED afterwards: like `pluck`, this is a
    # terminal, and a `WHERE id = 3` left on the relation would silently
    # narrow every later use of it to that one row. Popped BEFORE the
    # raise for the same reason — an exception a caller rescues must not
    # leave the relation altered.
    #
    # The messages are Rails 8.1's (`raise_record_not_found_exception!`)
    # minus the ` [WHERE ...]` suffix Rails appends for a scoped
    # relation: Rails renders it from Arel with `?` binds, while the
    # wheres here are SQL text with the values already filled in, so
    # the suffix could not match. An unscoped relation's message is
    # exactly Rails'.
    def find(id)
      # Rails compacts the ids first, so `find(nil)` has none: the
      # "without an ID" form, with no id, as `Base.find(nil)` raises.
      raise RecordNotFound.new("Couldn't find #{@model.name} without an ID", @model.name, @model.primary_key) if id.nil?
      return find_ids(id) if id.is_a?(Array)
      key = @model._cast_primary_key(id)
      prior_limit = @limit
      own_lists
      @wheres << "#{@table}.#{@model.primary_key} = #{ActiveRecord.adapter.escape_value(key)}"
      begin
        @limit = 1
        rows = load_records
        record = rows.length == 0 ? nil : rows[0]
      ensure
        @limit = prior_limit
        @wheres.pop
      end
      if record.nil?
        raise RecordNotFound.new("Couldn't find #{@model.name} with '#{@model.primary_key}'=#{id.inspect}", @model.name, @model.primary_key, id)
      end
      record
    end

    # Array form: deduplicate BEFORE the column cast, as Rails does.
    # Unordered relations follow the requested IDs (after slicing by
    # offset/limit); explicitly ordered relations follow their SQL order.
    # Read directly rather than through to_a: its loaded cache belongs to
    # the original relation and must neither mask nor remember this filter.
    def find_ids(ids)
      ids = ids.uniq
      return [] if ids.empty?
      return [find(ids[0])] if ids.length == 1
      prior_limit = @limit
      prior_offset = @offset
      prior_select = @select_sql
      expected = ids.length
      if @orders.empty?
        ids = ids[prior_offset || 0, prior_limit || ids.length] || []
        expected = ids.length
      else
        expected = prior_limit if !prior_limit.nil? && expected > prior_limit
        expected = ids.length - prior_offset if !prior_offset.nil? && ids.length - prior_offset < expected
      end
      keys = ids.map { |id| @model._cast_primary_key(id) }
      sql_ids = keys.map { |key| ActiveRecord.adapter.escape_value(key) }.join(", ")
      own_lists
      @wheres << (keys.empty? ? "1=0" : "#{@table}.#{@model.primary_key} IN (#{sql_ids})")
      begin
        if @orders.empty?
          @limit = nil
          @offset = nil
        end
        @select_sql = "#{prior_select}, #{@table}.#{@model.primary_key}" unless prior_select.nil?
        rows = load_records
      ensure
        @limit = prior_limit
        @offset = prior_offset
        @select_sql = prior_select
        @wheres.pop
      end
      if rows.length != expected
        listed = ids.map { |each_id| each_id.inspect }.join(", ")
        raise RecordNotFound.new("Couldn't find all #{Inflector.pluralize_word(@model.name, 2)} with '#{@model.primary_key}': (#{listed}) (found #{rows.length} results, but was looking for #{expected}).", @model.name, @model.primary_key, ids)
      end
      if @orders.empty?
        keys.map { |key| rows.find { |row| row.id == key } }
      else
        rows
      end
    end

    # A TERMINAL, so its predicate is POPPED — the same rule `find`
    # above spells out, and omitting it here cost campfire its entire
    # room page. `find_messages` asks `messages.find_by(id:
    # params[:message_id])` to decide between two pagings and then
    # pages THE SAME relation; on a plain `/rooms/1` the id is nil, so
    # the probe left `WHERE id IS NULL` behind and `last_page` answered
    # zero rows against a room holding a hundred. The page still came
    # back 200, complete and well-formed, with an empty message list —
    # which is exactly the failure the app's own tests cannot see.
    #
    # `add_condition` answers whether it pushed — a nil or empty
    # condition pushes nothing — so the pop is guarded by that rather
    # than issued unconditionally.
    def find_by(conditions)
      pushed = add_condition(conditions, [], false)
      record = first
      @wheres.pop if pushed
      record
    end

    # `find_by!` — `find_by` that raises `RecordNotFound` on no match.
    def find_by!(conditions)
      record = find_by(conditions)
      raise RecordNotFound.new("Couldn't find #{@model.name}", @model.name, @model.primary_key) if record.nil?
      record
    end

    # `sole` (Rails 7.0) — the relation's one record: `RecordNotFound`
    # when it matches none, `SoleRecordExceeded` when it matches more.
    # Rails reads `first(2)` and checks the size, so one query of at
    # most two rows tells the three cases apart.
    #
    # A LOADED relation decides from the memoized `@records` instead of
    # re-querying — Rails' own `first(2)` reads the loaded Array once
    # `loaded?` — so a caller that already has the page in memory (an
    # eager-loaded association, a prior `each`) costs no second trip to
    # the database.
    #
    # Unloaded, `first_n` cannot serve the probe: it always asks for
    # its own `n`, which would override a SMALLER existing `@limit`
    # (`limit(1).sole` must probe 1 row, not 2, so two matching rows
    # settle as the first one rather than `SoleRecordExceeded`; a
    # `limit(0)` relation must probe 0 and always read as
    # `RecordNotFound`), and it unconditionally clears `@records`
    # afterward, which would discard a cache this branch never had to
    # begin with. So the probe borrows and restores `@limit` itself,
    # capped at the relation's own limit (nil meaning the usual 2,
    # clamped at 0 or above), and restores `@records` to nil — what it
    # was, since this branch only runs when the relation is unloaded —
    # rather than to whatever the probe's own LIMIT happened to cache.
    def sole
      loaded = @records
      unless loaded.nil?
        rows = loaded_sole_rows(loaded)
        raise RecordNotFound.new("Couldn't find #{@model.name}", @model.name, @model.primary_key) if rows.length == 0
        raise SoleRecordExceeded, "Wanted only one #{@model.name}" if rows.length > 1
        return rows[0]
      end
      prior_limit = @limit
      @limit = sole_probe_limit(prior_limit)
      rows = to_a
      @limit = prior_limit
      @records = nil
      raise RecordNotFound.new("Couldn't find #{@model.name}", @model.name, @model.primary_key) if rows.length == 0
      raise SoleRecordExceeded, "Wanted only one #{@model.name}" if rows.length > 1
      rows[0]
    end

    # `find_sole_by(conditions)` — Rails' `where(conditions).sole`. A
    # terminal, so its predicate is popped as in `find_by`; popped in an
    # `ensure` because `sole` raises on the two answers that are not one
    # record, and a caller that rescues must get the relation back as
    # it was.
    #
    # `add_condition` unconditionally clears `@records` — it has no way
    # to know in advance whether the new predicate still matches a
    # loaded set — so a relation that was loaded before this call loses
    # its memo the moment the temporary condition is pushed. The
    # `ensure` restores the ORIGINAL `@records` saved before that push,
    # not whatever `sole`'s own probe leaves behind, so a loaded
    # relation's records are intact afterward whether `sole` returns or
    # raises.
    def find_sole_by(conditions)
      prior_records = @records
      pushed = add_condition(conditions, [], false)
      begin
        record = sole
      ensure
        @wheres.pop if pushed
        @records = prior_records
      end
      record
    end

    # NO `new` HERE, AND NOT BY OVERSIGHT. Rails builds records through
    # a relation (`User.active_bots.new`), but under spinel this class's
    # constructor is already `sp_Relation_new`, so an instance method of
    # that name emits a second definition of the same C symbol and the
    # program does not compile (`conflicting types for
    # 'sp_Relation_new'`). One landed briefly and turned every spinel
    # job red. Both forms are served by a call-site rewrite in
    # lower::scope_chain instead — the association one inside a threaded
    # class-method body, the scope one on the relation receiver itself
    # (`User.active_bots.new` -> `User.new(User.active_bots
    # .scope_attributes)`). Ledgered in docs/pipeline/runtime.md.

    # `first_or_initialize` — the first matching row, or a new unsaved
    # record when there is none. The caller assigns the remaining
    # attributes before `save` (the write path this serves), so the built
    # record starts blank rather than pre-filled from the where-conditions.
    def first_or_initialize
      record = first
      record.nil? ? @model.new : record
    end

    # Rails' `find_or_create_by` — the first row matching `conditions`,
    # or a saved new record carrying them. campfire's `Search.record` is
    # `find_or_create_by(query: query).touch`, reached through
    # `user.searches`, and the SCOPE is the point: the created row must
    # belong to that user, which is what `scope_attributes` carries.
    #
    # The caller's own conditions go on the OUTSIDE of the merge, which
    # is Rails' order — an explicit value wins over the scope's. Same
    # rule `scope_chain::merge_scope_attributes` applies at lower time
    # for the plain constructors; here the merge is the runtime's,
    # because the query half needs the same conditions anyway.
    #
    # Reading `scope_attributes` AFTER the find is safe: `find_by`
    # appends to `@wheres`, and only `where_scope` ever writes the
    # create-seed slot.
    def find_or_create_by(conditions)
      record = find_by(conditions)
      return record if !record.nil?
      created = @model.new(scope_attributes.merge(conditions))
      created.save
      created
    end

    # ---- SQL composition --------------------------------------------

    def to_sql
      select_sql_with("#{@table}.*")
    end

    # This relation rendered as a CONDITION value — `where(id: other)`
    # and `excluding(other)` both put one relation inside another, and a
    # subquery must project exactly ONE column. Rails projects the
    # primary key when no explicit `select` was given; `to_sql`'s
    # `<table>.*` is right at top level and wrong here, and sqlite says
    # so out loud: "sub-select returns 5 columns - expected 1".
    def to_subquery_sql
      select_sql_with("#{@table}.#{@model.primary_key}")
    end

    # Shared body. `default_cols` is the projection when the caller
    # never said `select(...)`; an explicit one always wins.
    def select_sql_with(default_cols)
      cols = @select_sql.nil? ? default_cols : @select_sql
      distinct = @distinct ? "DISTINCT " : ""
      sql = "#{cte_prefix}SELECT #{distinct}#{cols} FROM #{from_source}"
      sql = "#{sql} #{@joins.join(" ")}" if @joins.length > 0
      sql = "#{sql} WHERE #{@wheres.join(" AND ")}" if @wheres.length > 0
      sql = "#{sql} GROUP BY #{@groups.join(", ")}" if @groups.length > 0
      sql = "#{sql} HAVING #{@havings.join(" AND ")}" if @havings.length > 0
      sql = "#{sql} ORDER BY #{@orders.join(", ")}" if @orders.length > 0
      if !@limit.nil?
        sql = "#{sql} LIMIT #{@limit}"
      elsif !@offset.nil?
        # SQLite needs LIMIT even for offset-only pagination.
        sql = "#{sql} LIMIT -1"
      end
      sql = "#{sql} OFFSET #{@offset}" unless @offset.nil?
      sql
    end

    # JOIN + WHERE only — shared by count_sql / exists_sql so DISTINCT
    # and GROUP arms do not re-paste the ladder.
    def append_join_where(sql)
      sql = "#{sql} #{@joins.join(" ")}" if @joins.length > 0
      sql = "#{sql} WHERE #{@wheres.join(" AND ")}" if @wheres.length > 0
      sql
    end

    def append_group_having(sql)
      sql = "#{sql} GROUP BY #{@groups.join(", ")}" if @groups.length > 0
      sql = "#{sql} HAVING #{@havings.join(" AND ")}" if @havings.length > 0
      sql
    end

    def count_sql
      # DISTINCT / GROUP BY must count the result-set shape, not the
      # underlying rows (#343). Scalar `count` on a grouped relation
      # counts groups; the Hash form is `group_count`. LIMIT/OFFSET
      # stay off this SQL so a windowed relation still answers the
      # unwindowed total.
      if !@groups.empty?
        # Keep an explicit projection so HAVING can name selected
        # aliases (`select("COUNT(*) AS n").having("n > 1")`).
        cols = @select_sql.nil? ? "1 AS one" : @select_sql
        dist = @distinct ? "DISTINCT " : ""
        inner = append_group_having(
          append_join_where("#{cte_prefix}SELECT #{dist}#{cols} FROM #{from_source}")
        )
        return "SELECT COUNT(*) AS n FROM (#{inner}) AS __rh_count"
      end
      if @distinct
        inner = append_join_where(
          "#{cte_prefix}SELECT DISTINCT #{distinct_count_columns} FROM #{from_source}"
        )
        return "SELECT COUNT(*) AS n FROM (#{inner}) AS __rh_count"
      end
      append_join_where("#{cte_prefix}SELECT COUNT(*) AS n FROM #{from_source}")
    end

    # Projection COUNT(DISTINCT …) / `SELECT DISTINCT` use for a
    # distinct relation: an explicit `select` list when present, else
    # the primary key, qualified against the active FROM source.
    def distinct_count_columns
      if !@select_sql.nil?
        @select_sql
      elsif @from.nil?
        "#{@table}.#{@model.primary_key}"
      elsif @joins.length > 0
        # Bare pk is ambiguous once another joined table also has
        # that column (`from("parents").joins(...).distinct.count`).
        "#{from_source}.#{@model.primary_key}"
      else
        @model.primary_key.to_s
      end
    end

    # `SELECT 1 AS one … LIMIT n` for existence probes. Drops ORDER BY
    # and never hydrates. Keeps joins / WHERE / GROUP / HAVING / OFFSET.
    def exists_sql(n)
      # DISTINCT 1 collapses every row into one — `distinct.many?` would
      # always be false. Project the primary key so each distinct row
      # still occupies a probe slot under LIMIT n.
      cols = if @distinct
        "DISTINCT #{@table}.#{@model.primary_key} AS one"
      else
        "1 AS one"
      end
      sql = append_group_having(
        append_join_where("#{cte_prefix}SELECT #{cols} FROM #{from_source}")
      )
      # Respect an existing relation LIMIT: many?/one? on limit(1) must
      # not look past the window.
      lim = n
      unless @limit.nil?
        lim = @limit < n ? @limit : n
      end
      sql = "#{sql} LIMIT #{lim}"
      sql = "#{sql} OFFSET #{@offset}" unless @offset.nil?
      sql
    end

    # ---- helpers ----------------------------------------------------

    # `WITH RECURSIVE a AS (…), b AS (…) ` or nothing.
    def cte_prefix
      return "" if @ctes.empty?
      "WITH RECURSIVE #{@ctes.join(", ")} "
    end

    # The FROM source: `from(...)`'s, else this model's table.
    def from_source
      src = @from
      src.nil? ? @table : src
    end

    # A hash of conditions ANDed: `{is_deleted: false, user_id: 3}` ->
    # `is_deleted = 0 AND user_id = 3`. Array value -> `IN`, nil ->
    # `IS NULL`, nested Hash -> qualified `table.col = ...`.
    def hash_conditions(hash)
      parts = []
      @hash_columns = []
      hash.each do |key, val|
        if val.is_a?(Hash)
          val.each do |col, v|
            parts << column_predicate("#{key}.#{col}", v)
            @hash_columns << "#{key}."
          end
        else
          parts << column_predicate(key.to_s, val)
          @hash_columns << key.to_s
        end
      end
      parts.join(" AND ")
    end

    # Unqualified columns are qualified with this relation's own table
    # (as Rails does for hash conditions) so a condition survives `merge`
    # into a JOINed query where the bare name would be ambiguous —
    # `hidden_stories.user_id`, not `user_id`, after `joins(:hidings)`.
    def column_predicate(col, val)
      col = sql_ident(col)
      qcol = col.include?(".") ? col : "#{@table}.#{col}"
      if val.is_a?(Relation)
        # A relation value is Rails' subquery form —
        # `where(story_id: Tagging.where(...).select(:story_id))` →
        # `story_id IN (SELECT taggings.story_id FROM taggings …)`.
        # The inner relation renders inline; its values were escaped
        # as its own conditions were added. `to_subquery_sql`, not
        # `to_sql`: a relation with no explicit `select` must project
        # its primary key here, not every column.
        "#{qcol} IN (#{val.to_subquery_sql})"
      elsif val.is_a?(Array)
        # Record elements read their id — Rails' IN-of-records form
        # (`where(comment: comments)`, lobsters Vote.comments_flags);
        # scalar elements escape as-is.
        ids = val.map { |x| x.is_a?(Base) ? x.id : x }
        "#{qcol} IN (#{escape_list(ids)})"
      elsif val.nil?
        "#{qcol} IS NULL"
      elsif val.is_a?(Base)
        # A whole record under a (fk-renamed) key reads its id —
        # `where(user: user)` after the key lowered to `user_id`. The
        # static `v && v.id` narrowing was dropped in favor of this
        # runtime dispatch: hash values from untyped scope params can
        # be a record OR a collection, and only the runtime knows.
        "#{qcol} = #{ActiveRecord.adapter.escape_value(val.id)}"
      else
        "#{qcol} = #{ActiveRecord.adapter.escape_value(val)}"
      end
    end

    # For positional binds, split only the original `?` placeholders,
    # keeping escaped values verbatim. Preserve trailing empty parts so
    # missing binds leave their `?` intact. (`String#sub` used to search
    # already-inserted values, so a `?` or backslash in a bind leaked
    # into the next placeholder.)
    def substitute_binds(sql, args)
      first = args[0]
      return substitute_named_binds(sql, first) if args.length == 1 && first.is_a?(Hash)
      parts = sql.split("?", -1)
      result = parts[0].to_s
      index = 0
      while index < parts.length - 1
        if index < args.length
          result = result + ActiveRecord.adapter.escape_value(args[index])
        else
          result = result + "?"
        end
        result = result + parts[index + 1].to_s
        index += 1
      end
      result
    end

    # Rails' named binds: `having("COUNT(*) = :size AND … IN (:user_ids)",
    # size: 2, user_ids: [3, 5])`, campfire's direct-room lookup
    # (basecamp/once-campfire#310). Each `:name` the Hash answers is
    # replaced by its escaped value, and an Array value by a
    # comma-separated list. A `::` cast is left alone, as is a name the
    # Hash does not have. Before this, the placeholders reached SQLite
    # unbound and read as NULL: every lookup missed and every Ping
    # created a new direct room.
    def substitute_named_binds(sql, binds)
      out = ""
      i = 0
      n = sql.length
      while i < n
        ch = sql[i, 1].to_s
        prev = i > 0 ? sql[i - 1, 1].to_s : ""
        nxt = sql[i + 1, 1].to_s
        if ch == ":" && prev != ":" && nxt != ":" && named_bind_start?(nxt)
          j = i + 1
          j += 1 while j < n && named_bind_char?(sql[j, 1].to_s)
          name = sql[i + 1, j - i - 1].to_s
          key = name.to_sym
          if binds.key?(key)
            value = binds[key]
            out = out + (value.is_a?(Array) ? escape_list(value) : ActiveRecord.adapter.escape_value(value))
            i = j
            next
          end
        end
        out = out + ch
        i += 1
      end
      out
    end

    def named_bind_start?(ch)
      (ch >= "a" && ch <= "z") || (ch >= "A" && ch <= "Z") || ch == "_"
    end

    def named_bind_char?(ch)
      named_bind_start?(ch) || (ch >= "0" && ch <= "9")
    end

    def escape_list(vals)
      out = []
      vals.each { |v| out << ActiveRecord.adapter.escape_value(v) }
      out.join(", ")
    end

    # Rails' `sanitize_limit`: `Integer(n)`. An integer, or a String
    # spelling one (surrounding space allowed), is that integer; a Float
    # truncates; anything else is the ArgumentError `Integer()` raises —
    # never text in the LIMIT clause.
    def sql_limit(n)
      return n.to_i if n.is_a?(Float)
      text = n.to_s.strip
      unless text.match?(/\A[+-]?\d+\z/)
        raise ArgumentError, "invalid value for Integer(): \"" + n.to_s + "\""
      end
      text.to_i
    end

    # An `order` hash's direction: Rails' `VALID_DIRECTIONS`, else the
    # ArgumentError `validate_order_args` raises.
    def order_direction(dir)
      d = dir.to_s
      return d.upcase if d == "asc" || d == "desc" || d == "ASC" || d == "DESC"
      raise ArgumentError, "Direction \"" + d + "\" is invalid. Valid directions are: " \
        "[:asc, :desc, :ASC, :DESC, \"asc\", \"desc\", \"ASC\", \"DESC\"]"
    end

    # An identifier written into SQL: `col` or `table.col`. This runtime
    # writes names bare, so anything else is ArgumentError rather than a
    # fragment.
    def sql_ident(name)
      c = name.to_s
      unless c.match?(/\A[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)?\z/)
        raise ArgumentError, "SQL identifier \"" + c + "\" is not a column name"
      end
      c
    end

    # An `order` hash's KEY. Same allowlist as `sql_ident`.
    def order_hash_column(col)
      sql_ident(col)
    end

    # Developer SQL like campfire's `order("LOWER(name)")` /
    # `order("LOWER(rooms.name)")`, plus the documented zero-arg
    # `RANDOM()` / `random()`. A request-steered fragment such as
    # `(SELECT 1)` or `SLEEP()` does not match.
    def order_fn_term?(c)
      return true if c == "RANDOM()" || c == "random()"
      c.match?(/\A[A-Za-z_][A-Za-z0-9_]*\([A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)?\)\z/)
    end

    # A string `order` argument: comma-separated `col` / `table.col`
    # with optional ASC/DESC. Static corpus strings (`"id desc"`,
    # `"category asc, tags.tag asc"`) pass; `order(params[:sort])` with
    # SQL does not.
    def order_string(s)
      bits = s.split(",")
      raise ArgumentError, "Order \"" + s + "\" is not a column name" if bits.length == 0
      parts = []
      bits.each do |bit|
        words = bit.strip.split(/\s+/)
        if words.length == 0 || words.length > 2
          raise ArgumentError, "Order \"" + s + "\" is not a column name"
        end
        col = words[0]
        term = order_fn_term?(col) ? col : order_hash_column(col)
        if words.length == 2
          parts << "#{term} #{order_direction(words[1])}"
        else
          parts << term
        end
      end
      parts.join(", ")
    end

    # `order(:col)` / `order("col DESC")` / `order(col: :desc)` /
    # `order(rooms: { updated_at: :desc })` — Rails' nested-hash form
    # for a table-qualified column (campfire's direct-room sidebar).
    # Nested values go through `sql_ident` + `order_direction` so a
    # request-steered hash cannot splice SQL.
    def order_term(p)
      if p.is_a?(Hash)
        format_order_hash(p)
      else
        order_string(p.to_s)
      end
    end

    # Isolated so `each` sees `Hash[Symbol, untyped]` keys as Symbol.
    # `order_term`'s Hash|String union does not narrow, and untyped
    # `col` interpolations blow the Bar B ceiling.
    def format_order_hash(h)
      parts = []
      h.each do |col, dir|
        if dir.is_a?(Hash)
          inner_col = dir.keys[0]
          parts << "#{sql_ident(col)}.#{sql_ident(inner_col)} #{order_direction(dir[inner_col])}"
        else
          parts << "#{sql_ident(col)} #{order_direction(dir)}"
        end
      end
      parts.join(", ")
    end
  end
end
