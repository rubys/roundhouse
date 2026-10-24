# frozen_string_literal: true
# `SQLite3::Database` — the sqlite3 gem's connection class, for the
# spinel binary, over the libsqlite3 FFI `runtime/db.rb` declares
# (`SQL`). The ruby family loads the gem itself under this file's path
# (`project::ruby_runtime_files`), so this is spinel's alone.
#
# The surface is the one campfire reaches (rubys/roundhouse#698):
#
# - `ResponseCache` keeps a second, READ-ONLY connection as an observer —
#   `SQLite3::Database.new(path, readonly: true)` — and reads
#   `get_first_value("PRAGMA data_version")`, which changes whenever ANY
#   connection (another process included) commits. It must be its own
#   connection: data_version is only comparable on the one that read it.
# - `SqliteWalCheckpoint` opens the block form, sets
#   `busy_handler_timeout = 1_000` and `execute`s
#   `PRAGMA wal_checkpoint(PASSIVE)`.
#
# As the gem does: `new` opens (READWRITE|CREATE, or READONLY) and raises
# `SQLite3::CantOpenException` when it cannot; the block form yields the
# database and closes it after; `execute` binds its `?` parameters,
# steps the first statement of its SQL and answers its rows, each an Array of column values (Integer,
# Float, String, nil); `get_first_row` / `get_first_value` read the first
# of those; a failed prepare or step raises `SQLite3::SQLException`
# (`SQLite3::BusyException` for SQLITE_BUSY), carrying sqlite's message.
# `busy_handler_timeout=` is sqlite's own busy timeout: it retries a
# locked database for that many milliseconds, which is what the gem's
# handler does (the gem's also releases Ruby's GVL; this lane has none).
#
# And the part a schema-owning app reaches (hindmost's conversation
# index): `execute_batch` runs every statement of its SQL, binding
# `bind_vars` to each one that has exactly that many `?`s, and answers
# the first row of the last statement (nil when it returns none);
# `transaction` runs `begin <mode> transaction` (deferred by default)
# and, with a block, commits when the block returns and rolls back when
# it raises, answering the block's value; `commit`, `rollback` and
# `transaction_active?` are the gem's.
module SQLite3
  class Exception < StandardError
  end

  class CantOpenException < SQLite3::Exception
  end

  class SQLException < SQLite3::Exception
  end

  class BusyException < SQLite3::Exception
  end

  class Database
    BUSY = 5

    def initialize(path, readonly: false)
      @closed = false
      @readonly = readonly
      @lock = Mutex.new
      flags = readonly ? SQL::OPEN_URI_READONLY : SQL::OPEN_URI_RWC
      rc = 0
      @dbh = 0
      Db.prepare_lock.synchronize do
        rc = SQL.sqlite3_open_v2(path.to_s, SQL.sq_db_out, flags, nil)
        @dbh = SQL.read_ptr(SQL.sq_db_out)
      end
      if rc != SQL::OK
        # sqlite hands back a handle even when the open fails; it holds
        # the message and must still be closed.
        message = SQL.sqlite3_errmsg(@dbh)
        SQL.sqlite3_close(@dbh)
        @closed = true
        raise SQLite3::CantOpenException, message
      end
      if block_given?
        begin
          yield self
        ensure
          close
        end
      end
    end

    def self.open(path, readonly: false)
      SQLite3::Database.new(path, readonly: readonly)
    end

    def readonly?
      @readonly
    end

    def closed?
      @lock.synchronize { @closed }
    end

    def close
      @lock.synchronize do
        return nil if @closed
        rc = SQL.sqlite3_close(@dbh)
        raise_error(rc) if rc != SQL::OK
        @closed = true
      end
      nil
    end

    def busy_timeout=(milliseconds)
      @lock.synchronize { SQL.sqlite3_busy_timeout(@dbh, milliseconds) }
      milliseconds
    end

    def busy_handler_timeout=(milliseconds)
      @lock.synchronize { SQL.sqlite3_busy_timeout(@dbh, milliseconds) }
      milliseconds
    end

    # `bind_vars` fill the statement's `?` placeholders in order, as the
    # gem's `execute(sql, bind_vars = [])` does: one value or an Array of
    # them, each nil, an Integer, true/false (0/1), a Float (a REAL) or a
    # String (its bytes, NULs included). Anything else raises rather than
    # binding as text; the gem would bind a Hash by name.
    def execute(sql, bind_vars = [])
      binds = bind_vars.is_a?(Array) ? bind_vars : [bind_vars]
      @lock.synchronize do
        raise SQLite3::Exception, "cannot use a closed database" if @closed
        stmt = prepare_statement(sql)
        rows = []
        begin
          binds.each_with_index { |value, i| bind(stmt, i + 1, value) }
          while true
            rc = SQL.sqlite3_step(stmt)
            break if rc == SQL::DONE
            raise_error(rc) if rc != SQL::ROW
            rows.push(row(stmt))
          end
        ensure
          SQL.sqlite3_finalize(stmt)
        end
        rows
      end
    end

    def get_first_row(sql, *bind_vars)
      execute(sql, bind_vars).first
    end

    def get_first_value(sql, *bind_vars)
      first = execute(sql, bind_vars).first
      first.nil? ? nil : first[0]
    end

    # The gem's: every statement of `sql` in turn, each stepped once.
    # `bind_vars` bind to a statement only when it has exactly that many
    # `?`s (the others run with theirs unbound, i.e. NULL). Answers the
    # last statement's first row, nil when that statement returns none.
    def execute_batch(sql, bind_vars = [])
      result = nil
      @lock.synchronize do
        raise SQLite3::Exception, "cannot use a closed database" if @closed
        rest = sql.strip
        until rest.empty?
          head = first_statement(rest)
          rest = rest[head.length, rest.length - head.length].strip
          result = step_once(head, bind_vars, result)
        end
      end
      result
    end

    # `begin <mode> transaction`; with a block, the gem's shape: the block
    # gets the database, a StandardError it raises rolls back and
    # propagates, anything else (the block returning, a break) commits,
    # and the call answers the block's value. Without a block it answers
    # true and leaves the transaction open.
    def transaction(mode = :deferred)
      execute("begin #{mode} transaction")
      if block_given?
        abort = false
        begin
          yield self
        rescue StandardError
          abort = true
          raise
        ensure
          if abort
            rollback
          else
            commit
          end
        end
      else
        true
      end
    end

    def commit
      execute("commit transaction")
      true
    end

    def rollback
      execute("rollback transaction")
      true
    end

    def transaction_active?
      @lock.synchronize { SQL.sqlite3_get_autocommit(@dbh) == 0 }
    end

    private

    def prepare_statement(sql)
      rc = 0
      stmt = 0
      Db.prepare_lock.synchronize do
        rc = SQL.sqlite3_prepare_v2(@dbh, sql, -1, SQL.sq_stmt_out, nil)
        stmt = rc == SQL::OK ? SQL.read_ptr(SQL.sq_stmt_out) : 0
      end
      raise_error(rc) if rc != SQL::OK
      stmt
    end

    # One statement of a batch, stepped once: its first row, nil when it
    # returns none. Only comments or semicolons prepare to no statement;
    # the gem skips those, keeping the answer it had.
    def step_once(sql, bind_vars, previous)
      stmt = prepare_statement(sql)
      return previous if stmt == nil
      first = nil
      begin
        if bind_vars.length == SQL.sqlite3_bind_parameter_count(stmt)
          bind_vars.each_with_index { |value, i| bind(stmt, i + 1, value) }
        end
        rc = SQL.sqlite3_step(stmt)
        if rc == SQL::ROW
          first = row(stmt)
        elsif rc != SQL::DONE
          raise_error(rc)
        end
      ensure
        SQL.sqlite3_finalize(stmt)
      end
      first
    end

    # The leading statement of `sql`: up to the first `;` that sqlite
    # itself reads as ending a complete statement (not one inside a
    # string, a comment or a trigger body), or all of it when none does.
    def first_statement(sql)
      from = 0
      while true
        at = sql.index(";", from)
        return sql if at.nil?
        head = sql[0, at + 1]
        return head if SQL.sqlite3_complete(head) != 0
        from = at + 1
      end
    end

    def bind(stmt, idx, value)
      rc =
        if value.nil?
          SQL.sqlite3_bind_null(stmt, idx)
        elsif value.is_a?(Integer)
          SQL.sqlite3_bind_int64(stmt, idx, value)
        elsif value == true || value == false
          SQL.sqlite3_bind_int64(stmt, idx, value ? 1 : 0)
        elsif value.is_a?(Float)
          SQL.sqlite3_bind_double(stmt, idx, value)
        elsif value.is_a?(String)
          text = value.to_s
          SQL.sqlite3_bind_text(stmt, idx, text, text.bytesize, -1)
        else
          raise ArgumentError, "SQLite3::Database#execute: cannot bind #{value.class} here"
        end
      raise_error(rc) if rc != SQL::OK
    end

    def row(stmt)
      out = []
      n = SQL.sqlite3_column_count(stmt)
      i = 0
      while i < n
        t = SQL.sqlite3_column_type(stmt, i)
        if t == SQL::NULL_TYPE
          out.push(nil)
        elsif t == SQL::INTEGER_TYPE
          out.push(SQL.sqlite3_column_int64(stmt, i))
        elsif t == SQL::FLOAT_TYPE
          out.push(SQL.sqlite3_column_double(stmt, i))
        else
          out.push(SQL.sqlite3_column_text(stmt, i))
        end
        i += 1
      end
      out
    end

    def raise_error(rc)
      message = SQL.sqlite3_errmsg(@dbh)
      raise SQLite3::BusyException, message if rc == BUSY
      raise SQLite3::SQLException, message
    end
  end
end
