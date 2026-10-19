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
  # persisted state as Rails' `rolledback!` does.
  class EnlistedRecord
    attr_reader :record, :kind

    def initialize(record, kind)
      @record = record
      @kind = kind
    end
  end

  # Row set from `Connection#execute` / `#exec_query`. Mirrors the
  # slice of `ActiveRecord::Result` the corpus uses: `to_a`, `first`,
  # `each`, `rows`.
  class Result
    def initialize(rows)
      @rows = rows
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
    def transaction(mode = :deferred)
      if Db.transaction_open?
        name = "rh_raw_#{Db._txn_depth}"
        Db.exec("SAVEPOINT #{name}")
        begin
          result = yield
        rescue Exception => e
          begin
            Db.exec("ROLLBACK TO SAVEPOINT #{name}")
            Db.exec("RELEASE SAVEPOINT #{name}")
          rescue StandardError
            # SQLite may have ended the transaction itself already.
          end
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
      if value.nil?
        "NULL"
      elsif value.is_a?(Integer) || value.is_a?(Float)
        value.to_s
      elsif value.is_a?(TrueClass)
        "1"
      elsif value.is_a?(FalseClass)
        "0"
      else
        Db.escape_string(value.to_s)
      end
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
  class Base
    # Keep public input intact until schema-selected normalization, then raise
    # the same RecordNotFound for an invalid key or an absent record.
    # Reject nil before a key-typed adapter can coerce it to a real
    # zero/empty-string key.
    def self.find(id)
      raise RecordNotFound.new("Couldn't find #{name} without an ID", name, primary_key, id) if id.nil?
      result = _find_primary_key_input(id)
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
            Db.exec("RELEASE SAVEPOINT #{savepoint}") unless savepoint.nil?
            finished = true
            result
          rescue ActiveRecord::Rollback
            # Swallowed by the joined block that saw it, as in Rails: the
            # outer transaction carries on and commits.
            Db._txn_depth = depth
            unless savepoint.nil?
              begin
                Db.exec("ROLLBACK TO SAVEPOINT #{savepoint}")
                Db.exec("RELEASE SAVEPOINT #{savepoint}")
              rescue StandardError
                # SQLite may already have ended the whole transaction; the
                # Rollback still answers nil, as the outer ROLLBACK's does.
              end
              ActiveRecord::Base._rollback_enlisted(mark)
            end
            result = nil
          rescue Exception => e
            Db._txn_depth = depth
            unless savepoint.nil?
              begin
                Db.exec("ROLLBACK TO SAVEPOINT #{savepoint}")
                Db.exec("RELEASE SAVEPOINT #{savepoint}")
              rescue StandardError
                # SQLite may already have ended the whole transaction (see
                # the outer ROLLBACK below); that must not hide `e`.
              end
              ActiveRecord::Base._rollback_enlisted(mark)
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
          rescue ActiveRecord::Rollback
            # Rails' quiet way out: roll back, raise nothing, answer nil.
            rolled_back = true
            Db._txn_depth = 0
            Db.exec("ROLLBACK")
            ActiveRecord::Base._rollback_enlisted(0)
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
              ActiveRecord::TransactionHooks.rolled_back
              raise e
            end
            # The records' commit callbacks first, then the held jobs,
            # as Rails enqueues after them. A job that fails to enqueue
            # (a Sidekiq push to a down Redis) raises after both, so no
            # callback is skipped or left to fire at a later COMMIT.
            begin
              ActiveRecord::Base._commit_enlisted
            ensure
              ActiveRecord::TransactionHooks.committed
            end
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
    def self.upsert(attrs, unique_by: nil, on_duplicate: nil, returning: nil, record_timestamps: nil)
      upsert_all([attrs], unique_by: unique_by, on_duplicate: on_duplicate, returning: returning, record_timestamps: record_timestamps)
    end

    # The predicate of the unique index on `columns` (sorted, joined with
    # ", ") when that index is partial, else "". The lowering overrides it
    # for a model whose table has one. `upsert_all` adds it to the
    # conflict target, as Rails does: SQLite matches a partial index only
    # through its `WHERE`.
    def self._conflict_predicate(columns)
      ""
    end

    # `Model.upsert_all(rows, …)` → SQLite's
    # `INSERT … ON CONFLICT (target) DO UPDATE SET …`.
    #
    # The conflict target is `unique_by` when given, else the model's
    # `primary_key` — which is why that had to become a real per-model
    # value rather than an assumed `id`. Lobsters' Keystore is the case
    # in point: conflicting on `id` would insert a fresh autoincrement
    # row every call and then trip the UNIQUE index on `key`.
    #
    # `on_duplicate:` replaces the generated SET clause with a raw
    # fragment (`Arel.sql("value = value + 1")` — Arel.sql is the
    # identity here, so it arrives as a String). Otherwise every
    # non-conflict column is assigned from `excluded`, matching Rails.
    #
    # Timestamps as Rails 8.1's `record_timestamps` stamps them: an
    # insert fills every created/updated column a row leaves out, and an
    # update moves `updated_at` only when an assigned column changed; a
    # raw `on_duplicate:` is left as written.
    #
    # NOT Rails-complete, deliberately: no RETURNING (asking for it
    # raises rather than quietly handing back nothing). Returns the
    # affected-row count, the same currency `update_counters` deals in.
    def self.upsert_all(rows, unique_by: nil, on_duplicate: nil, returning: nil, record_timestamps: nil)
      return 0 if rows.length == 0
      if returning
        raise NotImplementedError, "#{name}.upsert_all: RETURNING is not supported"
      end

      cols = rows[0].keys
      target = unique_by.nil? ? primary_key : unique_by
      target_names = target.is_a?(Array) ? target.map { |c| c.to_s } : [target.to_s]

      # Not `ActiveSupport.db_now`: Rails' insert_all stamps with SQLite's own clock, to the millisecond.
      now = "STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW')"
      table_cols = schema_columns
      given = cols.map { |c| c.to_s }
      stamping = record_timestamps.nil? ? self.record_timestamps : record_timestamps
      stamps = [:created_at, :created_on, :updated_at, :updated_on].select { |c| stamping && table_cols.include?(c) && !given.include?(c.to_s) }
      tuples = rows.map do |row|
        values = cols.map { |c| ActiveRecord.adapter.escape_value(row[c]) }
        "(" + (values + stamps.map { |_c| now }).join(", ") + ")"
      end

      assigns = on_duplicate
      if assigns.nil?
        updatable = cols.reject { |c| target_names.include?(c.to_s) }
        # Every column IS the conflict target: there is nothing left to
        # assign, and `DO UPDATE SET` with an empty list is a syntax
        # error. Rails degrades to a no-op insert here too.
        unless updatable.length == 0
          unchanged = updatable.map { |c| "#{c} IS excluded.#{c}" }.join(" AND ")
          touches = [:updated_at, :updated_on].select { |c| stamps.include?(c) }.map do |c|
            "#{c} = (CASE WHEN (#{unchanged}) THEN #{table_name}.#{c} ELSE #{now} END)"
          end
          assigns = (touches + updatable.map { |c| "#{c} = excluded.#{c}" }).join(", ")
        end
      end
      action = assigns.nil? ? "DO NOTHING" : "DO UPDATE SET #{assigns}"

      predicate = _conflict_predicate(target_names.sort.join(", "))
      conflict_where = predicate == "" ? "" : " WHERE #{predicate}"

      sql = "INSERT INTO #{table_name} (#{(cols + stamps).join(", ")}) VALUES #{tuples.join(", ")}" \
            " ON CONFLICT (#{target_names.join(", ")})#{conflict_where} #{action}"
      ActiveRecord.adapter.execute_ddl(sql)
      ActiveRecord.adapter.changes
    end

    # `self.record_timestamps=` — Rails class-attribute toggling auto
    # timestamp stamping. Kept nowhere: `fill_timestamps` always stamps,
    # so analysis reports any assignment other than `true`; the per-call
    # `upsert_all(…, record_timestamps:)` is honored.
    def self.record_timestamps=(value)
      value
    end

    def self.record_timestamps
      true
    end

    # `record.update_column(name, value)` — write one attribute straight
    # to the row, skipping validations and callbacks. Sets the in-memory
    # value via the `[]=` indexer, then persists via the same adapter
    # path `save` uses.
    def update_column(name, value)
      self[name] = value
      _adapter_update
      true
    end

    # `record.update_columns(a: 1, b: 2)` — several attributes at once,
    # the same way: no validations, no callbacks, no timestamps.
    def update_columns(attributes)
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
    # they drive a Relation or `_adapter_*` directly.
    def self.where(conditions)
      ActiveRecord::Relation.new(self).where(conditions.to_h)
    end

    # Model Hash finders use the relation's NULL / IN predicates too.
    # `.to_h` matches `where` above: Hash is a no-op (nil / Array values
    # survive); non-Hash inputs raise rather than reach Relation's SQL path.
    def self.find_by(conditions)
      ActiveRecord::Relation.new(self).find_by(conditions.to_h)
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

    def self.joins(spec)
      all.joins(spec)
    end

    def self.left_joins(spec)
      all.left_joins(spec)
    end

    def self.left_outer_joins(spec)
      all.left_outer_joins(spec)
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
      ActiveRecord.adapter.select_rows(sql).map { |row| instantiate(row) }
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
      self.class.transaction do
        before_validation
        ok = valid?
        after_validation
        status = ok ? save_after_validation : false
        raise ActiveRecord::Rollback unless status
      end
      status
    end

    # `destroy` in a transaction too, for the same reason: a dependent
    # cascade or an `after_destroy` write that raises undoes the delete.
    # base.rb's body, restated as `save` is.
    def destroy
      return self unless persisted?
      self.class.transaction do
        before_destroy
        _adapter_delete
        @persisted = false
        @destroyed = true
        after_destroy
        _destroy_committed
      end
      self
    end

    # Rails' `touch` runs in a transaction too, and is an update as far
    # as the commit callbacks are concerned: `after_update_commit` and
    # `after_commit` fire for it (campfire's `Message::Searchable`
    # guards its index rewrite because boosts touch the message). The
    # bodies are base.rb's, restated for the transaction.
    def touch
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
      owner.touch if !owner.nil? && owner.persisted?
      nil
    end

    # The commit callbacks wait for the COMMIT: outside a transaction
    # they run now; inside one they are enlisted and run after the
    # outermost COMMIT (`_commit_enlisted`), or are dropped by a
    # ROLLBACK (`_rollback_enlisted`). Rails defers them the same way,
    # so an `after_create_commit` that enqueues a job never queues one
    # for a row a rollback removed.
    def _save_committed(was_new)
      kind = was_new ? 0 : 1
      if Db._txn_depth == 0
        _run_commit_callbacks(kind)
      else
        Db._txn_records << ActiveRecord::EnlistedRecord.new(self, kind)
      end
      nil
    end

    def _destroy_committed
      if Db._txn_depth == 0
        _run_commit_callbacks(2)
      else
        Db._txn_records << ActiveRecord::EnlistedRecord.new(self, 2)
      end
      nil
    end

    def _run_commit_callbacks(kind)
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

    # Run commit callbacks for the writes enlisted at and after `from`,
    # and drop them from the list. Writes enlisted earlier stay for the
    # parent transaction. Called for the outermost COMMIT (`from` 0)
    # and for a savepoint whose parent is not joinable.
    def self._commit_enlisted_from(from)
      records = Db._txn_records
      return nil if records.length <= from
      held = records[from, records.length - from]
      records.pop while records.length > from
      seen = []
      held.each do |entry|
        record = entry.record
        next if seen.any? { |other| other.equal?(record) }
        seen << record
        kind = entry.kind
        held.each do |other|
          next unless other.record.equal?(record)
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
      records = Db._txn_records
      return nil if records.length <= from
      undone = records[from, records.length - from]
      records.pop while records.length > from
      undone.each { |entry| entry.record._restore_after_rollback(entry.kind) }
      nil
    end

    # The id a rolled-back create was given stays on the record (Rails
    # puts back the pre-save nil): the unsaved sentinel differs by key
    # type, and a later save inserts the row afresh either way.
    def _restore_after_rollback(kind)
      if kind == 0
        @persisted = false
      elsif kind == 2
        @persisted = true
        @destroyed = false
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
  end
end
