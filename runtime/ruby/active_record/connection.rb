# frozen_string_literal: true
# Ruby-family model surfaces: raw-SQL connections and request-key finders.
#
# Rails hands back the adapter itself here; this runtime hands back a
# thin stateless facade over the per-target `Db` primitive shim — just
# the members the corpus reaches for when it drops below the Relation
# layer (lobsters' Keystore upserts, hand-written aggregate queries,
# `quote`/`quote_string` in SQL-building helpers). Statically
# resolvable by construction: fixed methods, no method_missing.
#
# Result rows are `Hash[String, untyped]` — raw SQL is the one place
# the row shape is genuinely dynamic (aliased aggregates, computed
# columns), so a typed bag is the honest contract rather than an
# avoidable erasure.
module ActiveRecord
  # Not an ivar on each class: Rails' class_attribute is inherited until a subclass sets its own, so the reader walks the ancestors through this table
  module TimestampSwitches
    @switches = { "ActiveRecord::Base" => true }

    def self.store(name, value)
      @switches[name] = value
      value
    end

    def self.lookup(name)
      @switches[name]
    end
  end

  # `raise ActiveRecord::Rollback` inside `transaction { }` rolls it back
  # quietly: the transaction rescues it and answers nil. Here beside
  # `transaction`, which only the ruby family has.
  class Rollback < StandardError
  end

  # Integer finder inputs must be validated before entering a typed adapter.
  # Keep validity separate from the Integer payload: every signed 64-bit
  # value, including INT64_MIN, is a valid payload, never a nil sentinel.
  # The current SQLite adapter has signed 64-bit integer keys even for
  # schema.rb `id: :integer`; narrower adapter widths need their own metadata.
  class IntegerKeyCast
    attr_reader :valid, :value

    # Validity is separate so every signed Integer can remain a payload.
    def initialize(valid, value)
      @valid = valid
      @value = value
    end

    # Preserve String keys; ordinary scalar inputs use their decimal text.
    def self.input_text(id)
      if id.nil?
        ""
      elsif id.is_a?(String)
        id
      else
        id.to_s
      end
    end

    # Apply Rails decimal-prefix rules and the adapter range before to_i.
    def self.parse(id)
      # An Integer already has a native representation. Keep it intact,
      # including MIN, rather than round-tripping through decimal formatting.
      if id.is_a?(Integer)
        return IntegerKeyCast.new(false, 0) if id < -9223372036854775808
        return IntegerKeyCast.new(false, 0) if id > 9223372036854775807
        return IntegerKeyCast.new(true, id)
      elsif id.is_a?(Float)
        # Numeric inputs truncate their VALUE, not a scientific-notation
        # String prefix. Use an exclusive 2**63 upper bound: Float(MAX)
        # rounds up to 2**63. These comparisons also reject NaN/infinities
        # before to_i, while preserving the exactly representable MIN.
        return IntegerKeyCast.new(false, 0) unless id >= -9223372036854775808.0 && id < 9223372036854775808.0
        return IntegerKeyCast.new(true, id.to_i)
      end
      text = input_text(id)
      i = number_start(text)
      negative = text[i, 1].to_s == "-"
      if negative || text[i, 1].to_s == "+"
        i += 1
      end
      digits = +""
      found = false
      while i < text.length
        char = text[i, 1].to_s
        if digit_value(char) >= 0
          digits = digits + char unless digits.empty? && char == "0"
          return IntegerKeyCast.new(false, 0) if digits.length > 19
          found = true
          i += 1
        elsif char == "_" && found && digit_value(text[i + 1, 1].to_s) >= 0
          # Ruby's decimal to_i accepts single underscores BETWEEN digits.
          # "1_0" is 10; "1__0" stops at the first underscore and is 1.
          i += 1
        else
          break
        end
      end
      return IntegerKeyCast.new(false, 0) unless found
      digits = "0" if digits.empty?
      limit = negative ? "9223372036854775808" : "9223372036854775807"
      return IntegerKeyCast.new(false, 0) if exceeds_limit(digits, limit)
      # Apply the sign BEFORE conversion: +2**63 is not representable,
      # while -2**63 is. Native to_i must never see an overflowing input.
      signed = negative ? "-" + digits : digits
      IntegerKeyCast.new(true, signed.to_i)
    end

    # Locate the optional sign/digits after Ruby-compatible ASCII whitespace.
    def self.number_start(text)
      i = 0
      while i < text.length
        char = text[i, 1].to_s
        if char != " " && char != "\t" && char != "\n" &&
           char != "\r" && char != "\v" && char != "\f"
          return i
        end
        i += 1
      end
      i
    end

    # Decimal ASCII digit, or -1; Unicode numeric characters are not IDs.
    def self.digit_value(char)
      digits = "0123456789"
      i = 0
      while i < digits.length
        return i if digits[i, 1].to_s == char
        i += 1
      end
      -1
    end

    # Compare normalized unsigned text without overflowing a native Integer.
    def self.exceeds_limit(digits, limit)
      return true if digits.length > limit.length
      return false if digits.length < limit.length
      i = 0
      while i < digits.length
        value = digit_value(digits[i, 1].to_s)
        bound = digit_value(limit[i, 1].to_s)
        return true if value > bound
        return false if value < bound
        i += 1
      end
      false
    end
  end

  # The edges of `Model.transaction`, for work that has to wait for the
  # COMMIT. No-ops here, so this file stays self-contained; the
  # Ruby-family job registry (runtime/job_registry.rb) redefines them to
  # hold the ActiveJob payloads of jobs that set
  # `enqueue_after_transaction_commit`, and a later definition wins.
  module TransactionHooks
    def self.began
      nil
    end

    def self.committed
      nil
    end

    def self.rolled_back
      nil
    end
  end

  # One write an open transaction holds the commit callbacks of: the
  # record, and what the write was (0 create, 1 update, 2 destroy).
  # `Base.transaction` runs the held callbacks after its outermost
  # COMMIT and drops them on a ROLLBACK, restoring the record's
  # persisted state as Rails' `rolledback!` does. Kind 3 is not a
  # write yet: a `touch_later` the outermost transaction applies just
  # before its COMMIT (`Base._flush_deferred_touches`), whose own touch
  # then enlists as an update.
  # Rails 7.2's `Model.current_transaction`: the open transaction, or
  # none (Rails' NullTransaction), as far as `after_commit` is concerned.
  # `after_commit` takes its work as a lambda, as ActiveJob's queue does:
  # `lower::transaction_after_commit` turns the app's block into one, so
  # the runtime types fully on every lane.
  class Transaction
    def after_commit(work)
      ActiveRecord::Base._after_commit(work)
    end
  end

  class EnlistedRecord
    attr_reader :record, :kind

    def initialize(record, kind)
      @record = record
      @kind = kind
    end
  end

  # Row set from `Connection#execute` / `#exec_query`. Mirrors the
  # slice of `ActiveRecord::Result` the corpus uses: `to_a`, `first`,
  # `each`, `rows`, `columns`, and the `affected_rows` a bulk write
  # (`insert_all`, `upsert_all`) answers.
  class Result
    # `affected_rows` below zero: the rows' own count, as for a read.
    def initialize(rows, affected_rows = -1)
      @rows = rows
      @affected_rows = affected_rows
    end

    # Rails 8's count of the rows the statement wrote. A write that
    # returned no rows passes its count; one that returned them (or a
    # read) answers how many there are.
    def affected_rows
      @affected_rows < 0 ? @rows.length : @affected_rows
    end

    def columns
      @rows.empty? ? [] : @rows[0].keys
    end

    # Rails' shape: each row an Array of its values, in column order
    # (`to_a` is the hashes). campfire reads ids an `insert_all!`
    # returned with `.rows.flatten`.
    def rows
      @rows.map { |row| row.values }
    end

    def to_a
      @rows
    end

    def first
      @rows.first
    end

    def each
      @rows.each do |row|
        yield row
      end
      @rows
    end

    def map
      mapped = @rows.map { |row| yield row }
      mapped
    end
  end

  # `connection.raw_connection` — see `Connection#raw_connection`.
  class RawConnection
    # SQLite3::Database#transaction: BEGIN in the given mode, the block,
    # COMMIT; ROLLBACK and re-raise on an exception.
    #
    # When SQLite is already in a transaction — the test shim's fixture
    # BEGIN, or a raw transaction this method already opened — a second
    # BEGIN fails ("cannot start a transaction within a transaction")
    # and a COMMIT would end the outer one. Nest a savepoint and
    # release it, or roll back to it, so the outer transaction stays
    # open. `Room::MessagesCount.with_immediate_write` takes this path
    # inside a fixture example, because that frame is hidden from
    # `transaction_open?` (see active_record_db_config.rb).
    # ROLLBACK TO plus RELEASE. A SQLite error here means the transaction
    # already ended; the caller's exception is the one that matters.
    def self._rollback_savepoint(name)
      begin
        Db.exec("ROLLBACK TO SAVEPOINT #{name}")
        Db.exec("RELEASE SAVEPOINT #{name}")
      rescue StandardError
        # SQLite may already have ended the transaction itself.
      end
      nil
    end

    def transaction(mode = :deferred)
      if Db.transaction_open?
        name = "rh_raw_#{Db._txn_depth}"
        Db.exec("SAVEPOINT #{name}")
        begin
          result = yield
        rescue Exception => e
          RawConnection._rollback_savepoint(name)
          raise e
        end
        Db.exec("RELEASE SAVEPOINT #{name}")
        result
      else
        Db.exec("BEGIN #{mode.to_s.upcase} TRANSACTION")
        begin
          result = yield
        rescue Exception => e
          begin
            Db.exec("ROLLBACK")
          rescue StandardError
            # SQLite may have ended the transaction itself already.
          end
          raise e
        end
        Db.exec("COMMIT")
        result
      end
    end
  end

  class Connection
    def self.shared
      @shared ||= Connection.new
    end

    # This runtime's only backend. Lobsters branches on this to pick
    # its upsert dialect; the SQLite arm is the one we execute.
    def adapter_name
      "SQLite"
    end

    # Rails `quote`: a full SQL literal, quotes included for strings.
    # `Db.escape_string` already wraps in single quotes (sqlite literal
    # syntax with '' doubling).
    def quote(value)
      Arel::Nodes.quote_scalar(value)
    end

    # Rails `quote_string`: escaped but UNquoted (callers embed it
    # inside their own quotes).
    def quote_string(str)
      str.gsub("'", "''")
    end

    # Run raw SQL, collecting every row as name→value. DML statements
    # simply produce zero rows. Delegates to the adapter's row loop
    # (which resolves column names once, not rows×cols times).
    def execute(sql)
      Result.new(ActiveRecord.adapter.select_rows(sql))
    end

    # `select_rows(sql)` — Rails' rows as Arrays of values, in column
    # order (campfire's tests read an `EXPLAIN QUERY PLAN`'s detail with
    # `select_rows(…).map(&:last)`).
    def select_rows(sql)
      ActiveRecord.adapter.select_rows(sql).map { |row| row.values }
    end

    # `select_value(sql)` — the first column of the first row, or nil
    # (campfire's room test counts FTS rows this way).
    def select_value(sql)
      select_rows(sql).dig(0, 0)
    end

    # Schema questions the app asks before touching a table (campfire's
    # `Room::MessagesCount.ensure!` checks both before installing its
    # triggers), answered from SQLite's own catalog.
    def data_source_exists?(name)
      !select_value("SELECT 1 FROM sqlite_master WHERE type IN ('table', 'view') AND name = #{quote(name.to_s)}").nil?
    end

    def column_exists?(table, column)
      select_rows("PRAGMA table_info(#{quote(table.to_s)})").any? { |row| row[1].to_s == column.to_s }
    end

    # The sqlite3 gem's connection under the adapter. Only its
    # `transaction(mode)` is reached: campfire repairs its counters in one
    # `BEGIN IMMEDIATE` write transaction.
    def raw_connection
      RawConnection.new
    end

    def exec_query(sql)
      execute(sql)
    end

    # `select_all(sql)` — the same rows under the name Rails' query
    # interface gives them (lobsters' TrafficHelper reads its activity
    # range this way).
    def select_all(sql)
      execute(sql)
    end

    # Rails' `exec_update(sql, name, binds)` / `exec_delete`: DML with
    # positional `?` binds, answering the rows affected. lobsters
    # recomputes a comment's score this way (`UPDATE comments SET …
    # confidence_order = unhex(?) WHERE id = ?`), on every comment and
    # vote save. The binds go through `sanitize_sql`, the escaping every
    # other raw-SQL entry here already uses; `name` is Rails' log label.
    def exec_update(sql, name = nil, binds = [])
      ActiveRecord.adapter.execute_ddl(Base.sanitize_sql([sql] + binds))
      ActiveRecord.adapter.changes
    end

    # lobsters' FullTextSearch drops an index row this way
    # (`DELETE FROM … where rowid = ?`).
    def exec_delete(sql, name = nil, binds = [])
      exec_update(sql, name, binds)
    end

    # lobsters' FullTextSearch adds an index row this way (`INSERT INTO …
    # (rowid, …) values (?, …)`). Rails answers a Result; an INSERT
    # selects nothing, so it is an empty one.
    def exec_insert(sql, name = nil, binds = [])
      exec_update(sql, name, binds)
      Result.new([])
    end
  end

  # The Base half of the Ruby-family model surface. Lives HERE (not base.rb)
  # deliberately: base.rb is transpiled into every strict target's
  # runtime via the runtime_loader tables, and this surface uses
  # begin/rescue (which several emitters don't lower yet) and the
  # Connection class (which those tables don't ship). This file is
  # walked only into the ruby-family trees, and active_record.rb
  # requires it AFTER base.rb so the reopen sees the real class.

  # Not in relation.rb: the strict targets' `Db` never raises RecordNotUnique.
  class Relation
    # Rails' `lock`: `FOR UPDATE`, or the clause given, ends the SELECT;
    # `lock(false)` takes it off. SQLite has no row locks, and Rails'
    # SQLite adapter writes none, so neither does this there.
    def lock(locking = true)
      rel = unloaded_spawn
      clause = ""
      if Db.adapter_name == "postgresql"
        if locking == true
          clause = "FOR UPDATE"
        elsif locking.is_a?(String)
          clause = locking
        end
      end
      rel.lock_clause!(clause)
    end

    def create_or_find_by(conditions)
      created = build(conditions)
      begin
        ActiveRecord::Base.transaction(requires_new: true) { created.save }
      rescue ActiveRecord::RecordNotUnique
        return find_by!(conditions)
      end
      created
    end

    def create_or_find_by!(conditions)
      created = build(conditions)
      begin
        ActiveRecord::Base.transaction(requires_new: true) { created.save! }
      rescue ActiveRecord::RecordNotUnique
        return find_by!(conditions)
      end
      created
    end
  end

  class Base
    # Rails' read-only records: one a `readonly` relation loaded, or one
    # `readonly!` marked, raises ReadOnlyRecord on save, destroy,
    # `update_column(s)` or `touch`.
    def readonly?
      @readonly == true
    end

    def readonly!
      @readonly = true
      self
    end

    def _raise_readonly_record_error
      raise ReadOnlyRecord.new("#{self.class.name} is marked as readonly")
    end

    def assign_attributes(attributes)
      unless attributes.is_a?(Hash)
        raise ArgumentError, "assign_attributes requires an attribute Hash"
      end
      attributes.each { |name, value| _assign_mass_attribute(name.to_s, value) }
      nil
    end

    def _assign_mass_attribute(name, value)
      _unknown_mass_attribute(name)
    end

    def _unknown_mass_attribute(name)
      raise ActiveModel::UnknownAttributeError.new(self, name, "unknown attribute '#{name}' for #{self.class.name}.")
    end

    # Ruby-family only, beside the other reopens here: strict targets have
    # no `Type::Value`, no untyped `self[name]` and no relation `all`.
    def read_attribute(name)
      self[name]
    end

    def write_attribute(name, value)
      self[name] = value
    end

    # Rails' `slice(*methods)`: the named attributes, String-keyed (Rails
    # wraps the Hash in a HashWithIndifferentAccess), from Symbol or
    # String names, flattened as Rails flattens them. Rails reads each
    # name through `public_send`; this runtime reads it through the `[]`
    # column indexer, as `read_attribute` does, so a name that is not a
    # column raises instead of answering a nil Rails would never give.
    # The indexer hands back a temporal column's stored text, so that
    # one is parsed into the Time its reader answers.
    def slice(*names)
      columns = self.class.schema_columns
      time_columns = self.class.schema_time_columns
      h = {}
      names.flatten.each do |n|
        name = n.to_sym
        unless columns.include?(name)
          raise NotImplementedError, "slice reads columns only; #{name} is not a column of #{self.class.name}"
        end
        h[name.to_s] = if time_columns.include?(name)
          ActiveSupport.present_db(ActiveSupport.parse_db_time(self[name]))
        else
          self[name]
        end
      end
      h
    end

    def saved_changes?
      !saved_changes.empty?
    end

    # Rails' `attribute_in_database`: the stored value, before any change
    # pending on this record. The changes are String-keyed; Rails takes a
    # Symbol as well.
    def attribute_in_database(name)
      attribute_was(name.to_s)
    end

    def self.schema_column_types
      {}
    end

    # Rails' `type_for_attribute(name).type`: the column's type name, nil
    # for a name that is no column.
    def self.type_for_attribute(name)
      ActiveRecord::Type::Value.new(schema_column_types[name.to_sym])
    end

    # Rails' `default_scoped`: `all` with the model's `default_scope`,
    # which this runtime does not model (a declared one is reported).
    def self.default_scoped
      all
    end

    # Keep public input intact until schema-selected normalization, then raise
    # the same RecordNotFound for an invalid key or an absent record.
    # Reject nil before a key-typed adapter can coerce it to a real
    # zero/empty-string key.
    def self.find(id)
      raise RecordNotFound.new("Couldn't find #{name} without an ID", name, primary_key, id) if id.nil?
      result = _default_scope_where.empty? ? _find_primary_key_input(id) : _find_in_scope(id)
      raise RecordNotFound.new("Couldn't find #{name} with '#{primary_key}'=#{id.inspect}", name, primary_key, id) if result.nil?
      result
    end

    # `find_by!` with Rails' readers on the error: the model and its key,
    # and no id. Here rather than in the shared base.rb, as with `find`
    # above: base.rb transpiles into the strict targets, whose emitters
    # render only the `raise Class, message` form.
    def self.find_by!(conditions)
      result = find_by(conditions)
      raise RecordNotFound.new("Couldn't find #{name}", name, primary_key) if result.nil?
      result
    end

    # Schema-generated models override this with one normalization path
    # before their scalar adapter. The fallback retains the legacy Ruby
    # contract for hand-written subclasses using the generic adapter.
    def self._find_primary_key_input(id)
      key = _cast_primary_key(id)
      return nil if key.nil?
      _adapter_find_by_id(key)
    end

    # `find` on a default-scoped model: the row only when the scope keeps it.
    def self._find_in_scope(id)
      key = _cast_primary_key(id)
      return nil if key.nil?
      all.find_by({ primary_key => key })
    end

    # Relation and legacy-adapter finders use this shared conversion.
    # The integer parser never converts an out-of-range decimal string.
    def self._cast_primary_key(id)
      return nil if id.nil?
      return IntegerKeyCast.input_text(id) if _string_primary_key
      cast = IntegerKeyCast.parse(id)
      cast.valid ? cast.value : nil
    end

    # Reject nil before a key-typed adapter can coerce it. Generated
    # models override `_exists_primary_key_input` with schema-selected
    # dispatch (same split as find) so Spinel never compiles String into
    # an Integer adapter slot. No argument asks for any row (Rails'
    # `exists?(conditions = :none)`); the optional argument is a splat
    # so "absent" and `nil` stay distinct without an untyped default.
    def self.exists?(*ids)
      return all.exists? if ids.empty?
      raise ArgumentError, "wrong number of arguments (given #{ids.length}, expected 0..1)" if ids.length > 1
      id = ids[0]
      return false if id.nil?
      # Conditions, not a key (Rails' `exists?(name: "x")`): the Relation
      # answers them, through the default scope when the model has one.
      return all.exists?(id) if id.is_a?(Hash)
      unless _default_scope_where.empty?
        key = _cast_primary_key(id)
        return false if key.nil?
        return all.where({ primary_key => key }).exists?
      end
      _exists_primary_key_input(id)
    end

    # Fallback for hand-written subclasses using the generic adapter.
    def self._exists_primary_key_input(id)
      key = _cast_primary_key(id)
      return false if key.nil?
      _adapter_exists_by_id?(key)
    end

    # Stateless facade — every member delegates straight to `Db`. One
    # instance per process, as Rails hands every caller (and every
    # model class) the same connection object: a test that stubs
    # `ActiveRecord::Base.connection.transaction_open?` must see that
    # stub through `connection_pool.with_connection`, which a fresh
    # instance per call would lose. Held on Connection, not on Base, so
    # `User.connection` and `ActiveRecord::Base.connection` agree.
    def self.connection
      ActiveRecord::Connection.shared
    end

    # Rails' `Model.sanitize_sql(["insert … values (?, ?)", a, b])` —
    # the array form, which is the one an app writes when it drops
    # below the Relation layer (campfire's `Message::Searchable` keeps
    # its FTS index this way). The head is the fragment and the tail
    # are its binds, escaped left to right, which is exactly what
    # `Relation`'s `where("… ?", x)` already does — same rule, reached
    # from the class instead of from a relation.
    #
    # HERE and not in `base.rb`: this file is the raw-SQL tier the
    # strict targets do not stage, and `sanitize_sql` belongs to it
    # twice over — it escapes through `ActiveRecord.adapter
    # .escape_value`, which is on the RBS adapter contract but NOT on
    # the narrower hand-written Go/Rust adapter interfaces, and its one
    # caller reaches it through `connection.execute`, which lives here
    # too. Put in base.rb it broke `go vet` on every fixture.
    #
    # SPLIT-and-interleave rather than the `sub`-per-bind loop
    # `Relation#substitute_binds` uses: `String#sub` is Ruby vocabulary
    # the Go emitter does not carry either ("sql.Sub undefined"), and
    # Relation gets away with it only because Go does not stage that
    # file. `split` every target speaks.
    #
    # The BIND COUNT is authoritative, not the placeholder count: Ruby
    # drops a trailing empty field, so `"… where rowid = ?"` splits to
    # one part, and appending a bind after each part while binds remain
    # reconstructs it exactly. A `?` with no bind left is dropped, which
    # is the same shape `substitute_binds` leaves it in. Question marks
    # inside single- or double-quoted SQL literals are not placeholders.
    # SQLite does not treat `\` as a string escape — a backslash is
    # literal, so `'\''` ends the quote at the second apostrophe.
    def self.sanitize_sql(statement)
      sql = statement[0].to_s
      out = ""
      bind = 1
      i = 0
      n = sql.length
      quote = nil
      while i < n
        c = sql[i, 1].to_s
        if !quote.nil?
          out = out + c
          quote = nil if c == quote
          i = i + 1
          next
        end
        if c == "'" || c == "\""
          quote = c
          out = out + c
          i = i + 1
          next
        end
        if c == "?"
          if bind < statement.length
            out = out + ActiveRecord.adapter.escape_value(statement[bind])
            bind = bind + 1
          end
          i = i + 1
          next
        end
        out = out + c
        i = i + 1
      end
      out
    end

    # Rails' array-form entry point (`sanitize_sql_array([...])`). Same
    # positional `?` interleave as `sanitize_sql` — apps that name the
    # `_array` form (raw upserts, hand-built fragments) must resolve
    # here (#400). Named Hash / `%s` binds are not modeled yet (would
    # raise the Bar B untyped residual via Hash[untyped] walks).
    def self.sanitize_sql_array(statement)
      sanitize_sql(statement)
    end

    # Rails' `sanitize_sql_for_order` on a String: the ORDER BY fragment
    # as written, unchanged. The Array form is not this: Rails 8.1 checks
    # its fragment with `disallow_raw_sql!`, which a fragment holding `?`
    # never passes (`UnknownAttributeReference`), so it stays unsupported.
    def self.sanitize_sql_for_order(condition)
      condition
    end

    # Rails' `current_transaction`. One database, so every model's is the
    # same; what `after_commit` asks of it is whether one is open.
    def self.current_transaction
      ActiveRecord::Transaction.new
    end

    # `current_transaction.after_commit { ... }`: run the work once the
    # outermost transaction commits (`Base.transaction` runs what is held
    # after the records' commit callbacks), drop it on a ROLLBACK, and
    # run it at once when none is open, as Rails' NullTransaction does.
    # Campfire's password change closes the user's live connections this
    # way: a later save in the same transaction could undo the change an
    # after_commit condition would read.
    #
    # A transaction that cannot be joined is no `current_transaction`
    # either (Rails 7.2's `user_transaction` is NULL_TRANSACTION for it):
    # a transactional test's own never commits, so work registered
    # directly under it runs at once, as Rails runs it.
    def self._after_commit(work)
      depth = Db._txn_depth
      if depth == 0 || !_txn_parent_joinable?(depth)
        work.call
      else
        Db._txn_callbacks << work
      end
      nil
    end

    # The held work registered at and after `from`, run (a COMMIT) or
    # dropped (a ROLLBACK).
    def self._run_after_commits_from(from)
      held = Db._txn_callbacks
      return nil if held.length <= from
      work = held[from, held.length - from]
      held.pop while held.length > from
      work.each { |w| w.call }
      nil
    end

    def self._drop_after_commits_from(from)
      held = Db._txn_callbacks
      held.pop while held.length > from
      nil
    end

    # Rails' `uncached { }`: the block's reads go to the database, not
    # the per-request replay cache, which comes back on after it.
    def self.uncached
      was = Db.query_cache_enabled?
      Db.query_cache_end if was
      begin
        yield
      ensure
        Db.query_cache_begin if was
      end
    end

    # Rails' `clear_query_caches_for_current_thread`: the replay cache
    # forgets what it holds and stays on (campfire's caching tests clear
    # it between two renders so the second one's queries are counted).
    def self.clear_query_caches_for_current_thread
      if Db.query_cache_enabled?
        Db.query_cache_end
        Db.query_cache_begin
      end
      nil
    end

    # `Model.transaction { ... }` — the block inside BEGIN/COMMIT, with
    # ROLLBACK + re-raise on any exception. A nested call JOINS the
    # outer transaction, as Rails' default (`requires_new: false`) does:
    # no BEGIN of its own, and an exception from it rolls back the whole
    # outer transaction when it reaches the outer block. The depth is
    # per thread, like the connection the transaction runs on.
    #
    # `isolation:`, `requires_new:`, and `joinable:` are Rails'
    # `DatabaseStatements#transaction` keyword options (the same three
    # `with_lock` forwards — see base.rb). `isolation:` is accepted and
    # ignored (no isolation levels). A nested `requires_new: true` runs
    # inside a SAVEPOINT, as Rails' does: an exception or
    # `ActiveRecord::Rollback` from it undoes only its own writes, and
    # the outer transaction carries on.
    #
    # `joinable: false` is Rails' fixture transaction (and
    # `pin_connection!`): a nested `transaction` cannot join it, so it
    # opens a SAVEPOINT even without `requires_new:`. Commit callbacks
    # for the writes inside that savepoint run when the savepoint
    # releases, because the parent is not joinable — the same rule as
    # Rails' `run_commit_callbacks = !current_transaction.joinable?`.
    # A `requires_new:` savepoint under a joinable parent still defers
    # callbacks to the parent. The test shim opens one non-joinable
    # transaction around each example unless the class sets
    # `use_transactional_tests` false.
    #
    # The depth reset on an exception is explicit, in the `rescue`
    # itself, rather than left to the `ensure` below: Spinel
    # (matz/spinel#8182) skips a `begin/rescue/ensure`'s ensure when the
    # exception leaves through the rescue (re-raised) or matches no
    # rescue, and a depth left at 1 turns every later transaction on the
    # thread into a "nested" one with no BEGIN — writes that a ROLLBACK
    # then cannot undo. The `ensure` still carries the SAME reset (CRuby
    # runs both; harmless, since both set the identical value) because a
    # non-local exit from the block — `return`/`break`/`next` out of
    # `transaction { ... }` — never reaches `rescue` at all, only
    # `ensure`. As Rails does, a non-local exit (no exception) COMMITS
    # the outermost transaction rather than rolling it back; `rolled_back`
    # tells `ensure` which happened so the two paths can't double-apply.
    #
    # Each branch keeps its `rescue` clauses in an inner `begin` and its
    # `ensure` in an outer one, rather than one `begin/rescue/ensure`.
    # Spinel inlines a yielding method and its block into the caller, so a
    # nested `transaction { ... }` lands inside the outer one's protected
    # body; an exception leaving that inner region's `ensure` then jumped
    # straight to the outer `ensure` and skipped the outer `rescue` when
    # both clauses sat on one `begin` — the outermost transaction ran its
    # COMMIT for an exception raised in a joined block. A `begin/rescue`
    # nested in a `begin/ensure` gets the right dispatch.
    #
    # The depth itself reads/writes through `Db._txn_depth`/`=` (see
    # runtime/ruby/db.rbs) rather than `Thread.current` directly: a raw
    # `Thread.current[:k]` read has no declared return type for this
    # narrow RBS-only probe (unlike the full pipeline's stdlib model),
    # so it type-checked as the unresolved `Var` here, not the honest
    # gradual `Untyped` — `Db`'s own contract keeps this method
    # concretely typed the same way its `Db.exec` calls already are.
    # `raise ActiveRecord::Rollback` inside the block rolls the
    # transaction back without the exception escaping, and the call
    # answers nil.
    #
    # The outermost transaction's edges are reported to
    # `TransactionHooks` (above). A nested block reports nothing.
    #
    # Bit `depth - 1` of `Db._txn_joinable_bits` records whether the
    # frame at that depth can be joined. A joined call copies its
    # parent's bit onto the new depth so the next nested call still
    # sees the real frame.
    def self._txn_parent_joinable?(depth)
      return false if depth <= 0
      bit = 1 << (depth - 1)
      (Db._txn_joinable_bits & bit) != 0
    end

    def self._txn_note_joinable(depth, joinable)
      bit = 1 << (depth - 1)
      bits = Db._txn_joinable_bits
      if joinable
        Db._txn_joinable_bits = bits | bit
      else
        Db._txn_joinable_bits = bits - (bits & bit)
      end
      nil
    end

    def self.transaction(isolation: nil, requires_new: nil, joinable: true)
      depth = Db._txn_depth
      if depth > 0
        parent_joinable = ActiveRecord::Base._txn_parent_joinable?(depth)
        wants_new = requires_new ? true : false
        open_savepoint = wants_new || !parent_joinable
        savepoint = open_savepoint ? "rh_savepoint_#{depth}" : nil
        # Callbacks run at this savepoint only when the parent cannot
        # be joined. A requires_new savepoint under a joinable parent
        # keeps them enlisted for that parent.
        run_callbacks = open_savepoint && !parent_joinable
        Db.exec("SAVEPOINT #{savepoint}") unless savepoint.nil?
        # The writes a ROLLBACK TO this savepoint undoes are the ones
        # enlisted after this mark.
        mark = Db._txn_records.length
        after_mark = Db._txn_callbacks.length
        new_depth = depth + 1
        Db._txn_depth = new_depth
        if open_savepoint
          ActiveRecord::Base._txn_note_joinable(new_depth, joinable)
        else
          ActiveRecord::Base._txn_note_joinable(new_depth, parent_joinable)
        end
        finished = false
        begin
          begin
            result = yield
            # Where this savepoint runs its writes' commit callbacks (an
            # unjoinable parent: a transactional test's), it is where
            # their deferred touches land too, before the RELEASE.
            ActiveRecord::Base._flush_deferred_touches_from(mark) if run_callbacks
            Db.exec("RELEASE SAVEPOINT #{savepoint}") unless savepoint.nil?
            finished = true
            result
          rescue ActiveRecord::Rollback
            # Swallowed by the joined block that saw it, as in Rails: the
            # outer transaction carries on and commits.
            Db._txn_depth = depth
            unless savepoint.nil?
              RawConnection._rollback_savepoint(savepoint)
              ActiveRecord::Base._rollback_enlisted(mark)
              ActiveRecord::Base._drop_after_commits_from(after_mark)
            end
            result = nil
          rescue Exception => e
            Db._txn_depth = depth
            unless savepoint.nil?
              RawConnection._rollback_savepoint(savepoint)
              ActiveRecord::Base._rollback_enlisted(mark)
              ActiveRecord::Base._drop_after_commits_from(after_mark)
            end
            raise e
          end
        ensure
          Db._txn_depth = depth
          # After the frame is popped, so a callback's own save sees
          # the parent. Spinel skips this ensure when the rescue
          # re-raises; `finished` is false on that path anyway.
          if finished && run_callbacks
            ActiveRecord::Base._commit_enlisted_from(mark)
            ActiveRecord::Base._run_after_commits_from(after_mark)
          end
        end
      elsif Db.transaction_open?
        # A transaction opened BELOW ActiveRecord (a raw `BEGIN`, or
        # `raw_connection.transaction`) owns the connection: a BEGIN of
        # our own would fail ("cannot start a transaction within a
        # transaction"), and its COMMIT or ROLLBACK is the caller's. The
        # block runs inside it, unwrapped, with commit callbacks running
        # at once as they do outside any transaction this runtime opened.
        begin
          yield
        rescue ActiveRecord::Rollback
          nil
        end
      else
        Db.exec("BEGIN")
        ActiveRecord::TransactionHooks.began
        Db._txn_depth = 1
        ActiveRecord::Base._txn_note_joinable(1, joinable)
        rolled_back = false
        begin
          begin
            result = yield
            # Rails' `before_committed!`: the touches deferred by
            # `touch_later` are written now, while the transaction is
            # still open, so a failed one rolls the whole write back.
            ActiveRecord::Base._flush_deferred_touches
            result
          rescue ActiveRecord::Rollback
            # Rails' quiet way out: roll back, raise nothing, answer nil.
            rolled_back = true
            Db._txn_depth = 0
            Db.exec("ROLLBACK")
            ActiveRecord::Base._rollback_enlisted(0)
            ActiveRecord::Base._drop_after_commits_from(0)
            ActiveRecord::TransactionHooks.rolled_back
            result = nil
          rescue Exception => e
            rolled_back = true
            Db._txn_depth = 0
            begin
              Db.exec("ROLLBACK")
            rescue StandardError
              # SQLite may already have ended the transaction itself (a
              # constraint violation it resolves by aborting the whole
              # transaction, not just the statement, does this) — the
              # ROLLBACK's own failure must not hide the real error below.
            end
            ActiveRecord::Base._rollback_enlisted(0)
            ActiveRecord::Base._drop_after_commits_from(0)
            ActiveRecord::TransactionHooks.rolled_back
            raise e
          end
        ensure
          if rolled_back
            nil
          else
            Db._txn_depth = 0
            begin
              Db.exec("COMMIT")
            rescue Exception => e
              # A failed COMMIT (SQLITE_BUSY, a deferred constraint) leaves
              # the transaction open: end it, and tell the hooks it rolled
              # back so nothing held for it is queued by a later COMMIT.
              begin
                Db.exec("ROLLBACK")
              rescue StandardError
                # Already ended; the COMMIT's error is the one to raise.
              end
              ActiveRecord::Base._rollback_enlisted(0)
              ActiveRecord::Base._drop_after_commits_from(0)
              ActiveRecord::TransactionHooks.rolled_back
              raise e
            end
            # The records' commit callbacks first, then the held jobs,
            # as Rails enqueues after them. A callback that raises ends
            # the COMMIT there, as in Rails' `commit_records`, whose raise
            # comes before the transaction's own callbacks, where
            # `after_all_transactions_commit` enqueues: the held jobs and
            # any `after_commit` work not yet run are dropped, and the
            # caller sees the callback's error. A job that fails to enqueue (a Sidekiq push to a down
            # Redis) raises after the callbacks have all run, so none is
            # skipped or left to fire at a later COMMIT.
            begin
              ActiveRecord::Base._commit_enlisted
              ActiveRecord::Base._run_after_commits_from(0)
            rescue Exception => e
              ActiveRecord::Base._drop_after_commits_from(0)
              ActiveRecord::TransactionHooks.rolled_back
              raise e
            end
            ActiveRecord::TransactionHooks.committed
          end
        end
      end
    end

    # `#with_lock` (`ActiveRecord::Locking::Pessimistic#with_lock`) —
    # locks the record, then runs the block inside a transaction,
    # answering the block's value. Dispatches through
    # `self.class.transaction` (same `self.class.` pattern as
    # `schema_columns` elsewhere in this file) rather than a bare
    # `transaction`: Base has no instance-level transaction delegator
    # like Rails' `ActiveRecord::Transactions#transaction`, only the
    # class one in connection.rb. An exception raised in the block
    # rolls the transaction back and re-raises, same as `transaction`
    # itself.
    #
    # Rails spells the trailing transaction options as a Hash
    # (`args.extract_options!`); here they are the same three keywords
    # `transaction` takes, and the lock clause is the optional
    # positional. Lives in this ruby-family reopen, not base.rb, because
    # it leans on `Base.transaction` (see the note in base.rb).
    def with_lock(lock = nil, isolation: nil, requires_new: nil, joinable: true)
      self.class.transaction(
        isolation: isolation,
        requires_new: requires_new,
        joinable: joinable
      ) do
        lock!(lock)
        yield
      end
    end

    # `Model.delete_all` for a model without the lowerer-emitted
    # override: the rows the DELETE removed, read off the statement
    # rather than counted beforehand. Ruby-family-only because
    # `changes` is — see base.rb's default.
    def self.delete_all
      return all.delete_all unless _default_scope_where.empty?
      ActiveRecord.adapter.delete_all(table_name)
      ActiveRecord.adapter.changes
    end

    # `Model.update_counters(id, col: delta, …)` — atomic column
    # increments (`col = col + delta`) on one row, skipping validations
    # and callbacks. Returns the affected-row count.
    def self.update_counters(id, counters)
      parts = []
      counters.each do |col, delta|
        parts.push("#{col} = #{col} + #{delta.to_i}")
      end
      sql = "UPDATE #{table_name} SET #{parts.join(", ")} WHERE id = #{ActiveRecord.adapter.escape_value(id)}"
      ActiveRecord.adapter.execute_ddl(sql)
      ActiveRecord.adapter.changes
    end

    # `Model.upsert(attrs, …)` — INSERT that folds into an UPDATE when it
    # collides, in one statement. Rails routes the single-row form
    # through `upsert_all`; so does this.
    def self.upsert(attrs, unique_by: nil, on_duplicate: nil, update_only: nil, returning: nil, record_timestamps: nil)
      upsert_all([attrs], unique_by: unique_by, on_duplicate: on_duplicate, update_only: update_only, returning: returning, record_timestamps: record_timestamps)
    end

    # The predicate of the unique index on `columns` (sorted, joined with
    # ", ") when that index is partial, else "". The lowering overrides it
    # for a model whose table has one. `upsert_all` adds it to the
    # conflict target, as Rails does: SQLite matches a partial index only
    # through its `WHERE`.
    def self._conflict_predicate(columns)
      ""
    end

    # The columns, joined with ", ", of the unique index named `name`,
    # else "". The lowering overrides it for a model whose table has a
    # unique index. A `unique_by:` naming an index means its columns, as
    # Rails' `InsertAll#find_unique_index_for` reads it: the conflict
    # target is the index's columns, never its name.
    def self._unique_index_columns(name)
      ""
    end

    # `unique_by:` as the columns an ON CONFLICT clause names.
    def self._conflict_target(unique_by)
      names = unique_by.is_a?(Array) ? unique_by.map { |c| c.to_s } : [unique_by.to_s]
      return names unless names.length == 1
      indexed = _unique_index_columns(names[0])
      indexed == "" ? names : indexed.split(", ")
    end

    # Rails reads every row's keys as Strings, so a row built from a
    # record's `attributes` with a Symbol key merged over it names each
    # column once, the later value winning.
    def self._string_keyed_rows(rows)
      keyed = []
      rows.each do |row|
        one = {}
        row.each { |name, value| one[name.to_s] = value }
        keyed.push(one)
      end
      keyed
    end

    # The database's clock, as Rails' `high_precision_current_timestamp`
    # stamps a bulk write with it: SQLite's to the millisecond,
    # PostgreSQL's in UTC (the zone this runtime reads a timestamp in).
    def self._database_now_sql
      return "(CURRENT_TIMESTAMP AT TIME ZONE 'UTC')" if Db.adapter_name == "postgresql"
      "STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW')"
    end

    # `left` and `right` are equal, NULL equal to NULL: SQLite's `IS`,
    # which PostgreSQL reads only before NULL/TRUE/FALSE.
    def self._same_value_sql(left, right)
      return "#{left} IS NOT DISTINCT FROM #{right}" if Db.adapter_name == "postgresql"
      "#{left} IS #{right}"
    end

    # The created/updated columns Rails 8.1's `record_timestamps` stamps
    # on a bulk write: the table's, less any the rows give.
    def self._bulk_stamps(given, record_timestamps)
      stamping = record_timestamps.nil? ? self.record_timestamps : record_timestamps
      table_cols = schema_columns
      [:created_at, :created_on, :updated_at, :updated_on].select { |c| stamping && table_cols.include?(c) && !given.include?(c.to_s) }
    end

    # `Model.upsert_all(rows, …)` → `INSERT … ON CONFLICT (target) DO
    # UPDATE SET …`, in SQLite's and PostgreSQL's shared syntax.
    #
    # The conflict target is `unique_by` when given (columns, or the name
    # of a unique index), else the model's `primary_key` — which is why
    # that had to become a real per-model value rather than an assumed
    # `id`. Lobsters' Keystore is the case in point: conflicting on `id`
    # would insert a fresh autoincrement row every call and then trip
    # the UNIQUE index on `key`.
    #
    # `on_duplicate:` replaces the generated SET clause with a raw
    # fragment (`Arel.sql("value = value + 1")`). `Arel.sql` answers an
    # `Arel::Nodes::SqlLiteral` whose `to_s` is that text, which the
    # interpolation below reads. Otherwise every non-conflict column is
    # assigned from `excluded`, matching Rails, or only those
    # `update_only:` names (a column or a list).
    #
    # Timestamps as Rails 8.1's `record_timestamps` stamps them: an
    # insert fills every created/updated column a row leaves out, and an
    # update moves `updated_at` only when an assigned column changed; a
    # raw `on_duplicate:` is left as written.
    #
    # NOT Rails-complete, deliberately: no RETURNING (asking for it
    # raises rather than quietly handing back nothing). Answers Rails'
    # `ActiveRecord::Result`, empty, its `affected_rows` the count.
    def self.upsert_all(rows, unique_by: nil, on_duplicate: nil, update_only: nil, returning: nil, record_timestamps: nil)
      return Result.new([], 0) if rows.length == 0
      if returning
        raise NotImplementedError, "#{name}.upsert_all: RETURNING is not supported"
      end

      keyed = _string_keyed_rows(rows)
      cols = keyed[0].keys
      target_names = _conflict_target(unique_by.nil? ? primary_key : unique_by)

      now = _database_now_sql
      stamps = _bulk_stamps(cols, record_timestamps)
      tuples = keyed.map do |row|
        values = cols.map { |c| ActiveRecord.adapter.escape_value(row[c]) }
        "(" + (values + stamps.map { |_c| now }).join(", ") + ")"
      end

      assigns = on_duplicate
      if assigns.nil?
        updatable = cols.reject { |c| target_names.include?(c) }
        unless update_only.nil?
          only = update_only.is_a?(Array) ? update_only.map { |c| c.to_s } : [update_only.to_s]
          updatable = updatable.select { |c| only.include?(c) }
        end
        # Every column IS the conflict target: there is nothing left to
        # assign, and `DO UPDATE SET` with an empty list is a syntax
        # error. Rails degrades to a no-op insert here too.
        unless updatable.length == 0
          unchanged = updatable.map { |c| _same_value_sql("#{table_name}.#{c}", "excluded.#{c}") }.join(" AND ")
          touches = [:updated_at, :updated_on].select { |c| stamps.include?(c) }.map do |c|
            "#{c} = (CASE WHEN (#{unchanged}) THEN #{table_name}.#{c} ELSE #{now} END)"
          end
          assigns = (touches + updatable.map { |c| "#{c} = excluded.#{c}" }).join(", ")
        end
      end
      action = assigns.nil? ? "DO NOTHING" : "DO UPDATE SET #{assigns}"

      predicate = _conflict_predicate(target_names.sort.join(", "))
      conflict_where = predicate == "" ? "" : " WHERE #{predicate}"

      sql = "INSERT INTO #{table_name} (#{(cols + stamps.map { |c| c.to_s }).join(", ")}) VALUES #{tuples.join(", ")}" \
            " ON CONFLICT (#{target_names.join(", ")})#{conflict_where} #{action}"
      ActiveRecord.adapter.execute_ddl(sql)
      Result.new([], ActiveRecord.adapter.changes)
    end

    # `Model.insert_all(rows, unique_by:, returning:, record_timestamps:)`
    # and its kin (insert_all.rb): one INSERT for every row, with neither
    # validations nor callbacks. `insert_all` and `insert` skip a row
    # that conflicts (`ON CONFLICT DO NOTHING`, on `unique_by:` when
    # given); `insert_all!` and `insert!` raise RecordNotUnique. The
    # lowering inlines the plain `insert_all(rows)` and
    # `insert_all!(rows, returning: [...])` (`lower::scope_chain`); the
    # other forms reach these.
    #
    # Answers Rails' `ActiveRecord::Result` of the RETURNING columns, one
    # row for each row written: the primary key unless `returning:` names
    # others (a list of columns, or a String of SQL), none for `false`.
    # Every row must carry the same keys; timestamps the rows leave out
    # are stamped as `upsert_all` stamps them.
    def self.insert_all(rows, unique_by: nil, returning: nil, record_timestamps: nil)
      _insert_rows(rows, true, unique_by, returning, record_timestamps)
    end

    def self.insert_all!(rows, returning: nil, record_timestamps: nil)
      _insert_rows(rows, false, nil, returning, record_timestamps)
    end

    def self.insert(attrs, unique_by: nil, returning: nil, record_timestamps: nil)
      _insert_rows([attrs], true, unique_by, returning, record_timestamps)
    end

    def self.insert!(attrs, returning: nil, record_timestamps: nil)
      _insert_rows([attrs], false, nil, returning, record_timestamps)
    end

    def self._insert_rows(rows, skip_duplicates, unique_by, returning, record_timestamps)
      return Result.new([], 0) if rows.length == 0
      keyed = _string_keyed_rows(rows)
      cols = keyed[0].keys
      keyed.each do |row|
        unless row.keys.sort == cols.sort
          raise ArgumentError, "All objects being inserted must have the same keys"
        end
      end

      now = _database_now_sql
      stamps = _bulk_stamps(cols, record_timestamps)
      tuples = keyed.map do |row|
        values = cols.map { |c| ActiveRecord.adapter.escape_value(row[c]) }
        "(" + (values + stamps.map { |_c| now }).join(", ") + ")"
      end

      conflict = ""
      if skip_duplicates
        if unique_by.nil?
          conflict = " ON CONFLICT DO NOTHING"
        else
          target_names = _conflict_target(unique_by)
          predicate = _conflict_predicate(target_names.sort.join(", "))
          conflict_where = predicate == "" ? "" : " WHERE #{predicate}"
          conflict = " ON CONFLICT (#{target_names.join(", ")})#{conflict_where} DO NOTHING"
        end
      end

      # `returning: false` asks for nothing; SQL (`Arel.sql("id, …")`) is
      # written as given.
      returned = ""
      if returning.nil?
        returned = primary_key.to_s
      elsif returning.is_a?(Array)
        returned = returning.map { |c| c.to_s }.join(", ")
      elsif returning != false
        returned = returning.to_s
      end

      sql = "INSERT INTO #{table_name} (#{(cols + stamps.map { |c| c.to_s }).join(", ")}) VALUES #{tuples.join(", ")}#{conflict}"
      return Result.new(_returned_rows("#{sql} RETURNING #{returned}")) unless returned == ""
      ActiveRecord.adapter.execute_ddl(sql)
      Result.new([], ActiveRecord.adapter.changes)
    end

    # The rows an `INSERT … RETURNING` wrote, read off the handle
    # `Db.exec_returning` answers. A write, so neither the request's
    # query cache nor its read snapshot answers it, as they would a
    # `select_rows`.
    def self._returned_rows(sql)
      stmt = Db.exec_returning(sql)
      rows = []
      begin
        ncols = Db.column_count(stmt)
        names = []
        i = 0
        while i < ncols
          names.push(Db.column_name(stmt, i))
          i += 1
        end
        while Db.step?(stmt)
          row = {}
          i = 0
          while i < ncols
            row[names[i]] = Db.column_value(stmt, i)
            i += 1
          end
          rows.push(row)
        end
      ensure
        Db.finalize(stmt)
      end
      rows
    end

    # Rails' `composite_primary_key?`: whether the model keys its records
    # on several columns. Every model here keys on one (a composite table
    # is keyed on the column its model declares), so the answer is false.
    def self.composite_primary_key?
      false
    end

    def self.record_timestamps=(value)
      TimestampSwitches.store(name, value)
    end

    def self.record_timestamps
      # Not a `superclass` walk: Spinel then mistypes an unrelated `File.read(path)` whose path may be a Rails::AppPath
      ancestors.each do |klass|
        found = TimestampSwitches.lookup(klass.name)
        return found unless found.nil?
      end
      true
    end

    # Not base.rb's answer: a strict target, which never loads this file, keeps no switch to read
    def stamps_timestamps?
      self.class.record_timestamps
    end

    # `record.update_column(name, value)` — write one attribute straight
    # to the row, skipping validations and callbacks. Sets the in-memory
    # value via the `[]=` indexer, then persists via the same adapter
    # path `save` uses.
    def update_column(name, value)
      _raise_readonly_record_error if readonly?
      self[name] = value
      _adapter_update
      true
    end

    # `record.update_columns(a: 1, b: 2)` — several attributes at once,
    # the same way: no validations, no callbacks, no timestamps.
    def update_columns(attributes)
      _raise_readonly_record_error if readonly?
      attributes.each { |name, value| self[name] = value }
      _adapter_update
      true
    end

    # `record.delete` — the row goes, with no callbacks (Rails'
    # `Persistence#delete`, beside `destroy`, which runs them).
    def delete
      _adapter_delete if persisted?
      @persisted = false
      @destroyed = true
      self
    end

    # Rails' `Base#as_json(only:)` attribute serializer, monomorphized:
    # the corpus reaches it only as `super(only: attrs)` inside a
    # model's own `as_json`, which the as_json_super lowering rewrites
    # to this call. String-keyed like Rails.
    #
    # `only:` NARROWS the attribute set — it does not define it. Rails
    # intersects it with the record's real attributes, so a name in the
    # list that isn't a column contributes nothing. lobsters' User
    # pushes `:homepage` (a typed_store attribute living inside the
    # `settings` column) beside `:about` (a real column); Rails emits
    # only `about`, and echoing the list verbatim added a
    # `"homepage": null` to every user in /hottest's JSON.
    #
    # Values come from the `[]` indexer, which hands back the STORED
    # text — right for every column except a temporal one, which Rails
    # renders as ISO8601 with three fractional digits in the app's zone.
    # `schema_time_columns` is the emitted fact that says which those
    # are.
    def _as_json_only(only)
      h = {}
      columns = self.class.schema_columns
      time_columns = self.class.schema_time_columns
      only.each do |k|
        next unless columns.include?(k)
        h[k.to_s] = if time_columns.include?(k)
          ActiveSupport.json_time(self[k])
        else
          self[k]
        end
      end
      h
    end

    # Rails-shape `where` fallback: a lazy Relation, so dynamic
    # call-sites chain off it (`klass.where(short_id: id).exists?` in
    # lobsters' ShortId, where `klass` is a class-valued attribute no
    # static lowering can resolve). Overrides base.rb's Array-returning
    # version, which stays for the strict-target runtime transpiles
    # (no Relation class in their tables); this file is walked only
    # into the ruby-family trees. Lowered call-sites don't land here —
    # they drive a Relation or `_adapter_*` directly. Every form Rails
    # takes forwards as written — a Hash, a SQL string, a string with
    # positional or named binds — as `Relation#where` already takes them.
    def self.where(condition = nil, *args)
      ActiveRecord::Relation.new(self).where(condition, *args)
    end

    # Model finders use the relation's NULL / IN predicates too. A SQL
    # string reaches Relation#find_by's raw-SQL path, as Rails'
    # `find_by("score > 6")` does.
    def self.find_by(conditions)
      ActiveRecord::Relation.new(self).find_by(conditions)
    end

    # `Model.find_or_initialize_by` / `find_or_create_by` /
    # `find_or_create_by!`: Rails delegates them to `all`, so the
    # default scope applies to the find and seeds the built record. The
    # lowered call sites (`lower::first_or_create`) inline the statement
    # forms; these answer the rest (a receiverless call inside a class
    # method, an `||` operand).
    def self.find_or_initialize_by(conditions)
      ActiveRecord::Relation.new(self).find_or_initialize_by(conditions.to_h)
    end

    def self.find_or_create_by(conditions)
      ActiveRecord::Relation.new(self).find_or_create_by(conditions.to_h)
    end

    def self.find_or_create_by!(conditions)
      ActiveRecord::Relation.new(self).find_or_create_by!(conditions.to_h)
    end

    def self.create_or_find_by(conditions)
      ActiveRecord::Relation.new(self).create_or_find_by(conditions.to_h)
    end

    def self.create_or_find_by!(conditions)
      ActiveRecord::Relation.new(self).create_or_find_by!(conditions.to_h)
    end

    # `Model.find_sole_by(attrs)` — Rails' `where(attrs).sole`; see
    # Relation#find_sole_by. `Model.sole` is the same on the whole table.
    def self.find_sole_by(conditions)
      ActiveRecord::Relation.new(self).find_sole_by(conditions.to_h)
    end

    def self.sole
      ActiveRecord::Relation.new(self).sole
    end

    # Rails-shape `all` fallback, same story as `where` above: a lazy
    # Relation so refiner chains the lowerers left dynamic
    # (`Category.all.order("category asc, tags.tag asc")…` on lobsters'
    # filters page) chain off it instead of crashing on base.rb's
    # eager-Array version. Lowered call-sites don't land here — the
    # arel pass claims a plain `Model.all` and the scope-chain
    # normalizer re-roots recognized chains onto
    # `ActiveRecord::Relation.new(Model)` directly.
    def self.all
      ActiveRecord::Relation.new(self)
    end

    def self.lock(locking = true)
      all.lock(locking)
    end

    # A Relation over the model with no default scope applied, which is
    # what Rails' UniquenessValidator queries (`finder_class.unscoped`).
    # With a block, Rails runs it with the model's default scope removed
    # from every relation built inside, and returns the block's value.
    def self.unscoped
      if block_given?
        tables = ActiveRecord::Relation.unscoped_tables
        tables << _table_sql
        value = nil
        begin
          value = yield
        ensure
          tables.pop
        end
        return value
      end
      ActiveRecord::Relation.new(self, false)
    end

    # The conditions of the model's `default_scope { where(…) }`, which
    # `Relation#initialize` adds; a scoped model's emit overrides this.
    def self._default_scope_where
      {}
    end

    # A default-scoped model is read only through its Relation, which
    # applies the scope (or not, inside `unscoped { }`): these class
    # forms answer from it rather than from the model's whole-table
    # adapter primitives, which base.rb's defaults use.
    def self.count
      return all.count unless _default_scope_where.empty?
      _adapter_count
    end

    def self.last
      return all.last unless _default_scope_where.empty?
      _adapter_last
    end

    def self.destroy_all
      return all.destroy_all unless _default_scope_where.empty?
      records = _adapter_all
      records.each { |r| r.destroy() }
      records
    end

    # SELECT 1 LIMIT 1. Strict targets keep COUNT in base.rb.
    def self.any?
      ActiveRecord::Relation.new(self).exists?
    end

    def self.none?
      !any?
    end

    # Rails-shape `none` fallback, same story as `where`/`all` above:
    # an empty Relation off the class. lobsters' `Search` reaches it
    # through a class-valued method (`searched_model.none`), which no
    # static lowering can resolve to one model (#132).
    def self.none
      ActiveRecord::Relation.new(self).none
    end

    # Class-side `Model.page(n)` / `Model.paginate(...)`: the same
    # page of a fresh Relation (`Relation#page` / `#paginate`).
    # Ruby-family-only for the reason `where` above is.
    def self.page(num = nil)
      ActiveRecord::Relation.new(self).page(num)
    end

    def self.paginate(num = nil, page: nil, per_page: nil)
      ActiveRecord::Relation.new(self).paginate(num, page: page, per_page: per_page)
    end

    # Class-side `Model.in_batches`, and a bare `in_batches` in a class method: `Relation#in_batches` off a fresh Relation.
    def self.in_batches(of: 1000, order: nil)
      relation = ActiveRecord::Relation.new(self)
      yield relation if block_given?
      relation
    end

    # Rails-shape `first` fallback, same story as `where`/`all` above:
    # spec/dynamic call sites reach the class method directly
    # (`Category.first` in lobsters' specs); lowered call sites don't
    # land here. `last` lives in base.rb over `_adapter_last` — this
    # one is ruby-family-only because Relation#first already carries
    # the ORDER BY <pk> ASC LIMIT 1 shape.
    def self.first
      ActiveRecord::Relation.new(self).first
    end

    # Not left to the lowerings: they re-root only the class-side shapes they recognize, and Rails' `Querying` hands every one of these to `all`.
    def self.delete_by(conditions)
      all.delete_by(conditions)
    end

    def self.destroy_by(conditions)
      all.destroy_by(conditions)
    end

    def self.distinct(value = true)
      all.distinct(value)
    end

    def self.eager_load(*names)
      all.eager_load(*names)
    end

    def self.excluding(*records)
      all.excluding(*records)
    end

    def self.find_each
      relation = all
      records = relation.loaded_records
      i = 0
      n = records.length
      while i < n
        yield records[i]
        i += 1
      end
      relation
    end

    def self.find_in_batches
      relation = all
      yield relation.loaded_records
      relation
    end

    def self.first_or_initialize
      all.first_or_initialize
    end

    def self.first!
      all.first!
    end

    def self.from(source)
      all.from(source)
    end

    def self.group(*parts)
      all.group(*parts)
    end

    def self.having(condition, *args)
      all.having(condition, *args)
    end

    def self.ids
      all.ids
    end

    def self.includes(*names)
      all.includes(*names)
    end

    def self.joins(*specs)
      all.joins(*specs)
    end

    def self.left_joins(*specs)
      all.left_joins(*specs)
    end

    def self.left_outer_joins(*specs)
      all.left_outer_joins(*specs)
    end

    def self.limit(n)
      all.limit(n)
    end

    def self.many?
      all.many?
    end

    def self.maximum(expr)
      all.maximum(expr)
    end

    def self.merge(other)
      all.merge(other)
    end

    def self.minimum(expr)
      all.minimum(expr)
    end

    def self.offset(n)
      all.offset(n)
    end

    def self.one?
      all.one?
    end

    def self.or(other)
      all.or(other)
    end

    def self.order(*parts)
      all.order(*parts)
    end

    def self.pick(col)
      all.pick(col)
    end

    def self.pluck(col)
      all.pluck(col)
    end

    def self.preload(*names)
      all.preload(*names)
    end

    def self.references(*names)
      all.references(*names)
    end

    def self.reorder(*parts)
      all.reorder(*parts)
    end

    def self.select(*specs)
      all.select(*specs)
    end

    def self.sum(expr)
      all.sum(expr)
    end

    def self.touch_all(name = nil)
      all.touch_all(name)
    end

    def self.unscope(where: nil)
      all.unscope(where: where)
    end

    def self.update_all(updates)
      all.update_all(updates)
    end

    def self.with_recursive(ctes)
      all.with_recursive(ctes)
    end

    def self.without(*records)
      all.without(*records)
    end

    # `User.take` — the Rails 8 authentication generator's tests set up
    # with it (`setup { @user = User.take }`), so the Rails Guides store
    # reaches here. Same row as `first` under SQLite; see Relation#take.
    def self.take
      ActiveRecord::Relation.new(self).first
    end

    # The Relation load path's Hash fallback for a hand-written model
    # (one with its own `instantiate` and no lowerer-emitted
    # `_hydrate_all`): the adapter's rows through `instantiate`, which
    # is what `Relation#to_a` did for every model before the typed
    # path. Ruby-family-only because `select_rows` is — the strict
    # targets' adapter contract stops at `all`/`find`/`count`, and
    # their models always carry the emitted override.
    def self._hydrate_all(sql)
      records = []
      # Through a local, as `Relation#load_records` explains.
      adapter = ActiveRecord.adapter
      adapter.each_row(sql) { |row| records << instantiate(row) }
      records
    end

    # base.rb's `_adapter_all` / `_adapter_find_by_id` defaults, for the
    # same hand-written models: the adapter's `all` / `find` queries,
    # each row yielded to `instantiate` rather than taken from the Array
    # `all`/`find` build (on Spinel that row reached every model's
    # `<Model>Row.from_raw` boxed). Lowerer-emitted models override both.
    def self._adapter_all
      records = []
      adapter = ActiveRecord.adapter
      adapter.each_row("SELECT * FROM #{table_name}") { |row| records << instantiate(row) }
      records
    end

    def self._adapter_find_by_id(id)
      found = nil
      adapter = ActiveRecord.adapter
      adapter.each_row("SELECT * FROM #{table_name} WHERE id = #{Db.escape_int(id)} LIMIT 1") { |row| found = instantiate(row) }
      found
    end

    # A plucked value as the record would hold it (`Relation#pluck`). A
    # model with a stored PostgreSQL array column overrides this to decode
    # that column's JSON text (`lower::array_columns`); every other value
    # is what the row held.
    def self.decode_plucked(column, value)
      value
    end

    # Not in base.rb: a block here would put one on every strict target's runtime.
    def _run_save_callbacks
      done = false
      value = catch(:abort) do
        before_save
        result = block_given? ? yield : true
        after_save
        done = true
        result
      end
      done ? value : false
    end

    def _run_create_callbacks
      done = false
      value = catch(:abort) do
        before_create
        result = block_given? ? yield : true
        after_create
        done = true
        result
      end
      done ? value : false
    end

    def _run_update_callbacks
      done = false
      value = catch(:abort) do
        before_update
        result = block_given? ? yield : true
        after_update
        done = true
        result
      end
      done ? value : false
    end

    def _run_destroy_callbacks
      done = false
      value = catch(:abort) do
        before_destroy
        result = block_given? ? yield : true
        after_destroy
        done = true
        result
      end
      done ? value : false
    end

    def _run_validation_callbacks
      done = false
      value = catch(:abort) do
        before_validation
        result = block_given? ? yield : true
        after_validation
        done = true
        result
      end
      done ? value : false
    end

    # Rails' `update_attribute`: one writer, then save WITHOUT
    # validations (validation callbacks skipped too) — save callbacks
    # still run. Specs use it to construct records a validation would
    # reject (lobsters' username-change history), so validating here
    # would break exactly the sites that reach for it. Enters save's
    # extracted post-validation half directly.
    def update_attribute(name, value)
      self[name] = value
      status = false
      self.class.transaction do
        status = save_after_validation
        raise ActiveRecord::Rollback unless status
      end
      status
    end

    # Rails' `save` runs inside a transaction (`with_transaction_
    # returning_status`): the row write, the callbacks and every write
    # they make (autosaved associations, an `after_save` that inserts
    # an index row) land together, or an exception from any of them
    # rolls them all back and propagates. A `false` from validation
    # rolls back what `before_validation` wrote. Inside an open
    # transaction the save joins it. The body is base.rb's `save`,
    # restated here rather than wrapped because `transaction` is a
    # ruby-family method the strict targets' base does not have.
    def save
      status = false
      @_save_halted = false
      self.class.transaction do
        # Ahead of valid?, as base.rb's save explains.
        _autosave_belongs_to
        ok = valid?
        status = ok ? save_after_validation : false
        raise ActiveRecord::Rollback unless status
      end
      status
    end

    # base.rb's body with before_validation able to halt it, as Rails'
    # can: the validations and after_validation are skipped and the
    # record is invalid with no errors. Rails clears the errors ahead
    # of that hook, so a halt also drops the previous run's.
    def valid?
      @errors = []
      @error_details = []
      return false if _halted_by_abort { before_validation }
      validate
      ok = @errors.empty?
      after_validation
      ok
    end

    def save!
      return self if save
      raise RecordNotSaved.new("Failed to save the record", self) if @_save_halted
      raise RecordInvalid, self
    end

    # base.rb's body with each before_* hook able to halt the save, as
    # Rails' before callbacks can; an after_* hook's `throw :abort` is not
    # caught, as in Rails, where it escapes as UncaughtThrowError.
    def save_after_validation
      _raise_readonly_record_error if readonly?
      if _halted_by_abort { before_save }
        @_save_halted = true
        return false
      end
      was_new = new_record?
      if was_new
        if _halted_by_abort { before_create }
          @_save_halted = true
          return false
        end
        fill_timestamps(true) if stamps_timestamps?
        @__id_before_insert = @id
        inserted_key = _adapter_insert
        self.id = inserted_key unless self.class.primary_key.nil?
        @persisted = true
        _enlist_write(0)
        __track_saved_changes(was_new)
        after_create
      else
        if _halted_by_abort { before_update }
          @_save_halted = true
          return false
        end
        fill_timestamps(false) if stamps_timestamps?
        _adapter_update
        _enlist_write(1)
        __track_saved_changes(was_new)
        after_update
      end
      after_save
      true
    end

    # The write is enlisted as soon as it's made, ahead of the after_*
    # hooks, as Rails adds the record to the transaction before the
    # save's callbacks: an exception one of them raises rolls the write
    # back, and the record is put back too (`_rollback_enlisted`), a
    # create a new record again. Its commit callbacks still wait for the
    # COMMIT. `save` always runs inside a transaction (its own, at
    # least), so the write is always enlisted here.
    def _enlist_write(kind)
      # The Dirty baseline before this transaction's first write of the
      # record, which a rollback puts back: its changes are pending again,
      # as Rails' `restore_transaction_record_state` leaves them.
      unless @__rollback_saved
        @__rollback_saved = true
        @__rollback_baseline = @__last_saved_attributes
      end
      _enlist_or_run(kind)
    end

    # The transaction that enlisted the record ended: nothing to put back.
    def _forget_rollback_state
      @__rollback_saved = false
      @__rollback_baseline = nil
      nil
    end

    # `destroy` in a transaction too, for the same reason: a dependent
    # cascade or an `after_destroy` write that raises undoes the delete.
    # base.rb's body, restated as `save` is.
    def destroy
      _raise_readonly_record_error if readonly?
      return self unless persisted?
      halted = false
      self.class.transaction do
        halted = _halted_by_abort { before_destroy }
        raise ActiveRecord::Rollback if halted
        _adapter_delete
        @persisted = false
        @destroyed = true
        after_destroy
        _destroy_committed
      end
      halted ? false : self
    end

    def destroy!
      return self if destroy
      raise RecordNotDestroyed.new("Failed to destroy #{self.class.name} with id=#{id}", self)
    end

    def _halted_by_abort
      done = false
      catch(:abort) do
        yield
        done = true
      end
      !done
    end

    # Rails' `touch` runs in a transaction too, and is an update as far
    # as the commit callbacks are concerned: `after_update_commit` and
    # `after_commit` fire for it (campfire's `Message::Searchable`
    # guards its index rewrite because boosts touch the message). The
    # bodies are base.rb's, restated for the transaction.
    def touch
      _raise_readonly_record_error if readonly?
      self.class.transaction do
        fill_timestamps(false)
        _adapter_touch
        _note_touched
        after_touch
        _save_committed(false)
      end
      true
    end

    def touch_written
      _raise_readonly_record_error if readonly?
      self.class.transaction do
        fill_timestamps(false)
        _adapter_update
        _note_touched
        after_touch
        _save_committed(false)
      end
      true
    end

    # The timestamp a touch wrote is saved, not pending: Rails clears
    # the touched attribute's change, so `changed?` stays false after a
    # rich text touches its record. Other pending changes stay pending.
    # Kept beside the snapshot rather than written into it: the snapshot
    # IS the Hash `attributes` built, and a write into it widens every
    # model's `attributes` literal to one value type (Spinel then types
    # `Article#attributes` as a poly hash its String-valued slot refuses).
    def _note_touched
      current = attributes
      @__touched_updated_at = current["updated_at"]
      nil
    end

    # The record a `belongs_to …, touch: true` touches when it was handed
    # over as an object rather than found from the key: Action Text's
    # `has_rich_text` sets it on the rich text it reads or builds, as
    # Rails' `inverse_of: :record` does (emit::ruby::library::
    # apply_rich_text_record_touch). `_touch_owner` runs from the
    # holder's `after_save` / `after_destroy`.
    def _adopt_touch_owner(owner)
      @__touch_owner = owner
      nil
    end

    def _touch_owner
      owner = @__touch_owner
      owner.touch_later if !owner.nil? && owner.persisted?
      nil
    end

    # Rails' `touch_later`, which `belongs_to …, touch: true` calls.
    # Inside a transaction the touch waits for the outermost COMMIT and
    # is written once per record: a message's rich text and the message
    # itself both touch the room, and Rails stamps the room once.
    # Waiting also keeps the UPDATE out of the middle of the save, which
    # holds SQLite's one write lock. Outside a transaction it is `touch`.
    def touch_later
      return touch if Db._txn_depth == 0
      records = Db._txn_records
      already = records.any? { |entry| entry.kind == 3 && entry.record.equal?(self) }
      records << ActiveRecord::EnlistedRecord.new(self, 3) unless already
      true
    end

    # Just before the outermost COMMIT: each record a `touch_later`
    # enlisted is touched, once. A touch fires `after_touch`, which can
    # defer its own parent's touch (Boost -> Message -> Room), so the
    # walk goes on until no deferred touch is left. The touch joins the
    # still-open transaction and enlists as an update, so its commit
    # callbacks run after the COMMIT with everyone else's.
    def self._flush_deferred_touches
      _flush_deferred_touches_from(0)
    end

    # The deferred touches enlisted at and after `from`: the outermost
    # COMMIT's (0), or a savepoint's that runs its own commit callbacks.
    def self._flush_deferred_touches_from(from)
      touched = []
      i = from
      while i < Db._txn_records.length
        entry = Db._txn_records[i]
        i += 1
        next unless entry.kind == 3
        record = entry.record
        next if touched.any? { |other| other.equal?(record) }
        touched << record
        record.touch
      end
      nil
    end

    # The commit callbacks wait for the COMMIT: outside a transaction
    # they run now; inside one they are enlisted and run after the
    # outermost COMMIT (`_commit_enlisted`), or are dropped by a
    # ROLLBACK (`_rollback_enlisted`). Rails defers them the same way,
    # so an `after_create_commit` that enqueues a job never queues one
    # for a row a rollback removed.
    def _save_committed(was_new)
      _enlist_or_run(was_new ? 0 : 1)
    end

    def _destroy_committed
      _enlist_or_run(2)
    end

    # Outside a transaction the commit callbacks run now. Inside one the
    # write is enlisted until the outermost COMMIT. Save and destroy
    # share this `Db` walk; only the callback kind differs.
    def _enlist_or_run(kind)
      if Db._txn_depth == 0
        _run_commit_callbacks(kind)
      else
        Db._txn_records << ActiveRecord::EnlistedRecord.new(self, kind)
      end
      nil
    end

    def _run_commit_callbacks(kind)
      # Committed: a later rollback has nothing of this one to put back.
      _forget_rollback_state
      if kind == 2
        after_destroy_commit
      else
        if kind == 0
          after_create_commit
        else
          after_update_commit
        end
        after_save_commit
      end
      after_commit
      nil
    end

    # The outermost COMMIT landed: each enlisted record's callbacks run
    # once, as Rails' `committed!` runs once for a record the
    # transaction wrote more than once. A create stays a create through
    # later updates; a destroyed record reports the destroy.
    def self._commit_enlisted
      _commit_enlisted_from(0)
    end

    # The writes enlisted at and after `from`, dropped from the list.
    # nil when that range is empty. Commit and rollback both start here.
    def self._take_enlisted(from)
      records = Db._txn_records
      return nil if records.length <= from
      held = records[from, records.length - from]
      records.pop while records.length > from
      held
    end

    # Run commit callbacks for the writes enlisted at and after `from`.
    # Writes enlisted earlier stay for the parent transaction. Called
    # for the outermost COMMIT (`from` 0) and for a savepoint whose
    # parent is not joinable.
    def self._commit_enlisted_from(from)
      held = _take_enlisted(from)
      return nil if held.nil?
      seen = []
      held.each do |entry|
        # A deferred touch is not a write; its touch enlisted one.
        next if entry.kind == 3
        record = entry.record
        next if seen.any? { |other| other.equal?(record) }
        seen << record
        kind = entry.kind
        held.each do |other|
          next unless other.record.equal?(record)
          next if other.kind == 3
          kind = other.kind if other.kind == 2 || (other.kind == 0 && kind == 1)
        end
        record._run_commit_callbacks(kind)
      end
      nil
    end

    # A ROLLBACK (to the savepoint whose mark is `from`, or of the
    # whole transaction for 0) undid the writes enlisted since: their
    # commit callbacks are dropped, and each record is put back the way
    # the database now has it -- a create is a new record again, a
    # destroy persisted again -- as Rails' `rolledback!` restores them.
    def self._rollback_enlisted(from)
      undone = _take_enlisted(from)
      return nil if undone.nil?
      undone.each { |entry| entry.record._restore_after_rollback(entry.kind) }
      nil
    end

    # A rolled-back create gets back the id it had before the insert, as
    # Rails puts back the pre-save nil, so it reads as unsaved again (its
    # held associations among them).
    def _restore_after_rollback(kind)
      if kind == 0
        @persisted = false
        @id = @__id_before_insert
      elsif kind == 2
        @persisted = true
        @destroyed = false
      end
      if @__rollback_saved
        @__last_saved_attributes = @__rollback_baseline
        _forget_rollback_state
      end
      nil
    end

    # Saved-change tracking (ActiveModel::Dirty subset) — the real
    # implementation behind base.rb's compile-surface stubs; see the
    # note there for why the diff is ruby-family-only. The snapshot
    # from the previous save (nil for a fresh instance, so a create
    # reports every attribute as [nil, value] — Rails' shape) diffs
    # against the post-write attributes; `save` calls this between the
    # row write and the after_* hooks, so callbacks observe the
    # finished save, matching Rails. A record hydrated from the DB has
    # no baseline yet, so its FIRST update over-reports;
    # baseline-at-hydration is future work.
    def __track_saved_changes(was_new)
      previous = @__last_saved_attributes
      current = attributes
      changes = {}
      current.each do |key, value|
        prev = previous.nil? ? nil : previous[key]
        changes[key] = [prev, value] if prev != value
      end
      @__last_saved_attributes = current
      @__touched_updated_at = nil
      @saved_changes = changes
      @id_previously_changed = was_new
      nil
    end

    def saved_changes
      @saved_changes || {}
    end

    # The Dirty baseline for a record that came from the DB. Without
    # it `__track_saved_changes` diffs the first update against a nil
    # snapshot and reports every column as `[nil, value]` — so
    # `<col>_previously_was` answered nil for all of them, which is how
    # campfire's `involvement_previously_was.inquiry.invisible?` found
    # this. The note above ("baseline-at-hydration is future work") is
    # what this closes.
    # The real value half: slot 0 of the `[prev, value]` pair. Bound to
    # a local before the nil test and the index, the same precaution
    # `saved_change_to_attribute?` documents. Ruby-family only — see
    # base.rb's stub for why the indexing cannot live there.
    def attribute_previously_was(name)
      pair = saved_changes[name]
      pair.nil? ? nil : pair[0]
    end

    def _note_hydrated
      @__last_saved_attributes = attributes
      @__touched_updated_at = nil
      nil
    end

    # The row's unselected schema columns — see base.rb's empty stub.
    # Rails answers `has_attribute?(col)` false for a column a partial
    # `select` left out, and lobsters' Token guard
    # (`if new_record? || has_attribute?(:token)`) is exactly that test:
    # a `User.select(*attrs)` without `token` must not mint one. Nothing
    # is recorded when the row carries every column, so the common path
    # allocates nothing.
    def _note_unloaded(row)
      missing = []
      self.class.schema_columns.each { |c| missing << c unless row.key?(c.to_s) }
      @__unloaded_columns = missing unless missing.empty?
      nil
    end

    def has_attribute?(name)
      return false unless self.class.schema_columns.include?(name)
      unloaded = @__unloaded_columns
      unloaded.nil? || !unloaded.include?(name)
    end

    # The pending diff: current attributes against the baseline the
    # last save (or hydration) left, in the same `[prev, value]` shape
    # as `saved_changes`. `save` runs validations and before_* hooks
    # BEFORE `__track_saved_changes` moves the baseline, so they see
    # what is about to be written, and after_* hooks see nothing
    # pending — Rails' order (`changes_applied` precedes the after
    # callbacks).
    #
    # A new record has no baseline, so every non-nil attribute reads as
    # pending. Rails compares against the column default instead, so a
    # column still holding its default (a `karma` of 0) over-reports
    # here; nil-default columns, which is what validations guard on,
    # agree.
    def changes_to_save
      previous = @__last_saved_attributes
      touched = @__touched_updated_at
      changes = {}
      attributes.each do |key, value|
        prev = previous.nil? ? nil : previous[key]
        prev = touched if key == "updated_at" && !touched.nil?
        changes[key] = [prev, value] if prev != value
      end
      changes
    end

    # Rails' `<col>_was`: the value before the pending change, or the
    # current value when nothing is pending. Both halves are the
    # `attributes` form, so a datetime column answers its raw stored
    # text, as `attribute_previously_was` already does.
    def attribute_was(name)
      pair = changes_to_save[name]
      pair.nil? ? attributes[name] : pair[0]
    end

    # Rails' `clear_<col>_change`: the column's current value becomes its
    # baseline, so it is no longer pending. Other columns stay as they were.
    def clear_attribute_change(name)
      baseline = @__last_saved_attributes
      baseline = baseline.nil? ? {} : baseline.dup
      baseline[name] = attributes[name]
      @__last_saved_attributes = baseline
      @__touched_updated_at = nil if name == "updated_at"
      nil
    end

    # Rails' `restore_<col>!`: a pending change is written back to its
    # original value and cleared.
    def restore_attribute!(name)
      if attribute_changed?(name)
        __send__("#{name}=", attribute_was(name))
        clear_attribute_change(name)
      end
      nil
    end

    # ---- cache_key / cache_version / cache_key_with_version ---------
    #
    # Lives HERE, not base.rb, for the same reason as the rest of this
    # file (see the class comment above): base.rb is transpiled into
    # every strict target's runtime, and `self[:updated_at]` below
    # compiles to whatever each target represents an untyped value as
    # (Rust: `serde_json::Value`), which has no `.to_i` — roundhouse
    # #1034's compare(rust)/compare-extra(kotlin,swift)/smoke(rust)/
    # smoke-extra(swift) all failed to compile their OWN runtime
    # library over this before it moved here. No real_blog app code on
    # a strict target calls `Base#cache_key_with_version` (ActiveRecord
    # there is base.rb's bare validation/errors surface, not a
    # queryable model), so moving it out costs strict targets nothing
    # they had.
    #
    # Placed at the END of this reopen, after every other method,
    # simply because this is where the rebase onto main landed it —
    # this block originally sat right after `_pend_preload`/
    # `_await_preload`, a deferred-preload mechanism main removed
    # (replaced by `Relation#preload_associations`/`_preload_<assoc>`)
    # between this PR's fork point and the rebase. The first rebase
    # attempt's conflict resolution wrongly kept `_pend_preload`/
    # `_await_preload` (their diff hunk's "ours" side was empty, which
    # read as "nothing to merge" rather than "main deleted this"), which
    # left `ActiveRecord::PendingPreload` referenced with no class
    # anywhere left to declare it — confirmed by grepping the whole
    # tree, `.rb`/`.rbs`/`.rs` alike, for `PendingPreload` and finding
    # only this file. That orphaned pair is gone now; nothing else in
    # the tree ever called them.
    #
    # Rails' `ActiveRecord::Integration` (`active_record/integration.rb`):
    # a single record's half of the same fragment-cache key Relation's
    # `cache_key_with_version` (`relation.rb`) answers for a collection.
    # A jbuilder `cache!` key naming one record — `json.cache!([
    # current_account, @widget]) do … end`, `current_account` a
    # `helper_method` answering a record — calls this the same way an
    # app controller or view does, and `cache!`'s `key` is a plain
    # positional parameter (jbuilder 2.15.1's `jbuilder_template.rb`),
    # so it is evaluated whether or not the controller is caching.
    #
    # Exact Rails parity except the timestamp rendering (see below):
    # `id` and `table_name` are real facts here, not a digest over SQL
    # text the way Relation's collection key has to approximate, so
    # there is nothing to trade off on the key/version SHAPE itself —
    # only on how the updated_at TIMESTAMP inside it is written.
    #
    # Rails always takes the `cache_versioning = true` shape (the
    # default since Rails 5.2; this runtime never models it as
    # configurable), under which `cache_key` carries no timestamp at
    # all and `cache_version` carries it alone:
    #   new record       cache_key "<table>/new",   cache_version nil
    #   saved record      cache_key "<table>/<id>",
    #                      cache_version "<updated_at>" (nil without an
    #                      updated_at column)
    #   cache_key_with_version   "<key>-<version>", or just <key> when
    #                      <version> is nil (an unsaved record, or no
    #                      updated_at column)
    def cache_key
      tbl = self.class.table_name
      return "#{tbl}/new" if new_record?
      rid = id
      "#{tbl}/#{rid}"
    end

    # Reads `updated_at` through `self[:updated_at]`, not the bare
    # per-model-synthesized `updated_at` reader: that reader is not a
    # method Base itself declares (every concrete model synthesizes its
    # own), so a bare call from this body has no declared return type
    # to seed from and came back an unresolved type variable in the
    # hand-RBS-typed `every_runtime_method_body_is_fully_typed` gate,
    # which allows the declared `Untyped` gradual escape but not a true
    # inference gap.
    #
    # `[]` is the established escape for exactly this (`fill_timestamps`,
    # base.rb, writes `self[:updated_at] = now` for the identical
    # reason, the write side of this same problem). Base's own `[]` is
    # typed `Str` in the catalog — a known, already-accepted
    # approximation ("Imprecise for non-string columns (`article[:id]`
    # is actually Int), but matches the existing declared shape",
    # `catalog/mod.rs`) for the STATIC type only: the CONCRETE per-model
    # `[]` the lowerer synthesizes (`model_to_library/schema.rs`)
    # answers the REAL column value through a `case` on the attribute
    # name, declared `Untyped` there — but for a `datetime`/`timestamp`
    # column (`ColumnSlot::index_read`, `index_reads_logical: false`:
    # only `date` reads its logical value through `[]`) that REAL value
    # is the RAW STORED TEXT, not a hydrated `Time` — the same storage
    # `@updated_at_raw` ivar `format_db_time` writes. `Str` is therefore
    # exactly what `self[:updated_at]` answers here, not merely an
    # approximation of it.
    #
    # Parse it with `ActiveSupport.parse_db_time` BEFORE calling `to_f`
    # (roundhouse #1034, CodeRabbit): a raw timestamp string's `to_f`
    # parses only its leading digits — `"2026-10-19 14:17:00".to_f` is
    # `2026.0`, the year alone, because `String#to_f` stops at the first
    # byte that cannot continue a float literal and `-` after 4 digits
    # does not qualify as a continuation. Every update within the same
    # YEAR then answered the identical `cache_version`, not merely the
    # same SECOND the first fix (below) addressed for the already-parsed
    # `Time` `Relation#compute_cache_version` aggregates through `MAX()`.
    #
    # `parsed.to_f` (seconds with a fractional part), not Rails'
    # `to_fs(:usec)` string — the same tradeoff `Relation#
    # compute_cache_version` documents, and the same fix (roundhouse
    # #1034, CodeRabbit: `to_i` truncated to whole seconds, so two
    # updates inside the same second answered the same
    # `cache_key_with_version`). `Time#to_f` is confirmed directly
    # against the installed Spinel compiler.
    def cache_version
      return nil unless has_attribute?(:updated_at)
      ts = self[:updated_at]
      return nil if ts.nil?
      parsed = ActiveSupport.parse_db_time(ts)
      parsed.nil? ? nil : parsed.to_f.to_s
    end

    def cache_key_with_version
      version = cache_version
      version.nil? ? cache_key : "#{cache_key}-#{version}"
    end
  end
end
