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
    # them, each nil, an Integer, true/false (0/1) or a String (its bytes,
    # NULs included). Anything else raises rather than binding as text —
    # the gem would bind a Float as a REAL, and a Hash by name.
    def execute(sql, bind_vars = [])
      binds = bind_vars.is_a?(Array) ? bind_vars : [bind_vars]
      @lock.synchronize do
        raise SQLite3::Exception, "cannot use a closed database" if @closed
        rc = 0
        stmt = 0
        Db.prepare_lock.synchronize do
          rc = SQL.sqlite3_prepare_v2(@dbh, sql, -1, SQL.sq_stmt_out, nil)
          stmt = rc == SQL::OK ? SQL.read_ptr(SQL.sq_stmt_out) : 0
        end
        raise_error(rc) if rc != SQL::OK
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

    private

    def bind(stmt, idx, value)
      rc =
        if value.nil?
          SQL.sqlite3_bind_null(stmt, idx)
        elsif value.is_a?(Integer)
          SQL.sqlite3_bind_int64(stmt, idx, value)
        elsif value == true || value == false
          SQL.sqlite3_bind_int64(stmt, idx, value ? 1 : 0)
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
