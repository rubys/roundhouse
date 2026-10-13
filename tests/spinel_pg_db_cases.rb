# Db-level cases for runtime/spinel/db_pg.rb against a live PostgreSQL.
# tests/spinel_pg_db.rs compiles this file with Spinel next to db_pg.rb
# and runs it with DATABASE_URL and SPINEL_PG_SCHEMA set. Every table
# lives in that throwaway schema, dropped at the end.
require_relative "db_pg"

# The app runtime supplies ActiveSupport.present (zone conversion);
# parse_db_time only needs it to hand the Time back.
module ActiveSupport
  def self.present(t)
    t
  end
end

# The app runtime defines these (runtime/ruby/active_record/errors.rb);
# pg_errors.rb raises them.
module ActiveRecord
  class RecordNotUnique < StandardError
  end

  class ValueTooLong < StandardError
  end

  # Real ActiveRecord::Rollback (runtime/ruby/active_record/connection.rb);
  # ar_transaction below rescues it exactly as the real
  # ActiveRecord::Base.transaction does.
  class Rollback < StandardError
  end
end

def sqlstate_of
  code = ""
  begin
    yield
  rescue PG::Error => e
    code = e.result.error_field(PG::PG_DIAG_SQLSTATE).to_s
  end
  code
end

# A copy, not a require, of ActiveRecord::Base.transaction's control
# flow (runtime/ruby/active_record/connection.rb) — connection.rb pulls
# in the rest of the ActiveRecord surface (the adapter, Relation, schema
# columns) that this narrow Db-level gate does not set up (see the file
# header: "the test drives the Db surface directly"). The part #693
# added Db._txn_depth for, and the part the Postgres regression broke,
# is reproduced verbatim so the cases below exercise the real shape.
def ar_transaction
  depth = Db._txn_depth
  if depth > 0
    Db._txn_depth = depth + 1
    begin
      result = yield
    rescue ActiveRecord::Rollback
      Db._txn_depth = depth
      result = nil
    rescue Exception => e
      Db._txn_depth = depth
      raise e
    ensure
      Db._txn_depth = depth
    end
  else
    Db.exec("BEGIN")
    Db._txn_depth = 1
    rolled_back = false
    begin
      result = yield
    rescue ActiveRecord::Rollback
      rolled_back = true
      Db._txn_depth = 0
      Db.exec("ROLLBACK")
      result = nil
    rescue Exception => e
      rolled_back = true
      Db._txn_depth = 0
      begin
        Db.exec("ROLLBACK")
      rescue StandardError
        nil
      end
      raise e
    ensure
      if rolled_back
        nil
      else
        Db._txn_depth = 0
        Db.exec("COMMIT")
      end
    end
  end
end

$checks = 0

# Report on stderr and exit, rather than raise: an exception's message
# can come back stale on Spinel once its storage is reused, and a
# failing gate should name the check that failed.
def fail_check(text)
  $stderr.puts "FAIL " + text
  exit 1
end

def check(label, ok)
  fail_check(label) if !ok
  $checks += 1
  nil
end

def check_int(label, want, got)
  fail_check(label + ": want " + want.to_s + ", got " + got.to_s) if want != got
  $checks += 1
  nil
end

def check_str(label, want, got)
  fail_check(label + ": want " + want.inspect + ", got " + got.inspect) if want != got
  $checks += 1
  nil
end

def check_raises(label, fragment)
  message = ""
  begin
    yield
  rescue StandardError => e
    message = e.message
  end
  if !message.include?(fragment)
    fail_check(label + ": want an error containing " + fragment.inspect + ", got " + message.inspect)
  end
  $checks += 1
  nil
end

# ── URL parsing (no server involved) ──

cfg = PgConfig.new("postgres://wid%40get:p%3Ass%20w@db.example:6543/gadget_db")
check_str("url host", "db.example", cfg.host)
check_int("url port", 6543, cfg.port)
check_str("url user", "wid@get", cfg.user)
check_str("url password", "p:ss w", cfg.password)
check_str("url database", "gadget_db", cfg.database)
cfg = PgConfig.new("postgresql://widget@db.example/")
check_str("url user only", "widget", cfg.user)
check_str("url database defaults to user", "widget", cfg.database)
check_raises("url parameters refused", "not supported") { PgConfig.new("postgres://db.example/app?sslmode=require") }
check_raises("url scheme checked", "postgres://") { PgConfig.new("mysql://db.example/app") }
check_raises("unconfigured", "not configured") { Db.prepare("SELECT 1") }

url = ENV.fetch("DATABASE_URL", "")
schema = ENV.fetch("SPINEL_PG_SCHEMA", "")
raise "DATABASE_URL and SPINEL_PG_SCHEMA must be set" if url == "" || schema == ""
Db.configure(url, pool_size: 2)
s = schema + "."
Db.exec("DROP SCHEMA IF EXISTS " + schema + " CASCADE")
Db.exec("CREATE SCHEMA " + schema)

# Each case is its own method: one top-level function holding every
# case makes the C compiler crawl.

def case_01(s, schema)
  Db.exec("CREATE TABLE " + s + "widgets (id bigserial PRIMARY KEY, name text NOT NULL, " +
          "active boolean, weight float8, made_at timestamp(6), gadget_id integer)")
  Db.exec("CREATE TABLE " + s + "gadgets (id uuid PRIMARY KEY DEFAULT gen_random_uuid(), label text)")
  check_int("DDL changes", 0, Db.changes)
  check_raises("last_insert_rowid needs an INSERT", "was not an INSERT") { Db.last_insert_rowid }
  nil
end

def case_02(s, schema)
  # ── Inline writes through the escape helpers ──

  Db.exec("INSERT INTO " + s + "widgets (name, active, weight, made_at, gadget_id) VALUES (" +
          Db.escape_string("Widget's \"first\"") + ", " + Db.escape_bool(true) + ", " +
          Db.escape_float_opt(1.5) + ", '2024-01-31 12:34:56.123456', " + Db.escape_int(7) + ")")
  check_int("insert changes", 1, Db.changes)
  check_int("first serial key", 1, Db.last_insert_rowid)
  Db.exec("INSERT INTO " + s + "widgets (name, active, weight, made_at, gadget_id) VALUES (" +
          Db.escape_string_opt("gädget ✓ \\ back") + ", " + Db.escape_bool_opt(false) + ", " +
          Db.escape_float_opt(nil) + ", NULL, " + Db.escape_int_opt(nil) + ")")
  check_int("second serial key", 2, Db.last_insert_rowid)
  Db.exec("INSERT INTO " + s + "widgets (name, active, weight, gadget_id) VALUES (" +
          Db.escape_string("third") + ", " + Db.escape_bool_opt(nil) + ", 3.25, " + Db.escape_int_opt(7) + ")")
  check_int("third serial key", 3, Db.last_insert_rowid)
  # The inserted key comes from the INSERT's own table: never another
  # table's sequence, whatever this session drew last.
  Db.exec("INSERT INTO " + s + "gadgets (label) VALUES ('after serial')")
  check_raises("uuid key after a serial insert", "no serial or identity primary key") { Db.last_insert_rowid }
  Db.exec("CREATE TABLE " + s + "sprockets (id serial PRIMARY KEY, note text)")
  Db.exec("CREATE TABLE " + s + "cogs (cog_id integer GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, note text)")
  Db.exec("ALTER SEQUENCE " + s + "cogs_cog_id_seq RESTART WITH 100")
  qs = "\"" + schema + "\"."
  Db.exec("INSERT INTO " + s + "sprockets (note) VALUES ('a')")
  check_int("serial key, interleaved 1", 1, Db.last_insert_rowid)
  Db.exec("INSERT INTO " + qs + "\"cogs\" (note) VALUES ('a')")
  check_int("identity key under its own column name", 100, Db.last_insert_rowid)
  Db.exec("INSERT INTO " + s + "sprockets (note) VALUES ('b')")
  check_int("serial key, interleaved 2", 2, Db.last_insert_rowid)
  Db.exec("  insert into " + s + "widgets (name) VALUES ('fourth')")
  check_int("widgets keep their own sequence", 4, Db.last_insert_rowid)
  Db.exec("DELETE FROM " + s + "widgets WHERE id = 4")
  Db.exec("INSERT INTO " + qs + "\"cogs\" (note) VALUES ('b')")
  check_int("identity key, interleaved", 101, Db.last_insert_rowid)
  check_str("insert target, quoted", qs + "\"cogs\"", Db.insert_target("INSERT INTO " + qs + "\"cogs\" (note) VALUES (1)"))
  check_str("insert target, escaped quote", "\"a\"\"b\"", Db.insert_target("INSERT INTO \"a\"\"b\" DEFAULT VALUES"))
  check_str("insert target, not an insert", "", Db.insert_target("UPDATE widgets SET id = 1"))
  check_str("escape_int_list", "1, 2, 3", Db.escape_int_list([1, 2, 3]))
  check_str("escape_int_list empty", "NULL", Db.escape_int_list([]))
  check_str("escape_string_opt nil", "NULL", Db.escape_string_opt(nil))
  nil
end

def case_03(s, schema)
  # ── Bound reads ──

  # Column metadata before the first step runs the statement (PostgreSQL
  # describes columns with a result), so it comes after the binds.
  h = Db.prepare("SELECT id, name, active, weight, made_at, gadget_id FROM " + s + "widgets WHERE id = $1")
  Db.bind_int(h, 1, 1)
  check_int("column_count before step", 6, Db.column_count(h))
  check_str("column_name before step", "gadget_id", Db.column_name(h, 5))
  check_raises("bind after the statement ran", "already ran") { Db.bind_int(h, 1, 1) }
  Db.finalize(h)

  h = Db.prepare("SELECT id, name, active, weight, made_at, gadget_id FROM " + s + "widgets WHERE id = $1")
  check_raises("bind index is one-based", "one-based") { Db.bind_int(h, 0, 1) }
  Db.bind_int(h, 1, 1)
  check("bound int finds the row", Db.step?(h))
  check_int("column_int", 1, Db.column_int(h, 0))
  check_str("quotes round-trip", "Widget's \"first\"", Db.column_text(h, 1))
  check("column_bool true", Db.column_bool(h, 2))
  check_int("bool through column_int", 1, Db.column_int(h, 2))
  check("column_float", Db.column_float(h, 3) == 1.5)
  check_int("column_int_opt present", 7, Db.column_int_opt(h, 5))
  t = ActiveSupport.parse_db_time(Db.column_text_opt(h, 4))
  check("timestamp parses", !t.nil?)
  if !t.nil?
    check_str("timestamp fields", "2024-01-31 12:34:56", t.strftime("%Y-%m-%d %H:%M:%S"))
    check_int("timestamp micros", 123456, t.usec)
  end
  check("one row only", !Db.step?(h))
  check("stays done", !Db.step?(h))
  Db.finalize(h)

  h = Db.prepare("SELECT name, active, weight, made_at, gadget_id FROM " + s + "widgets WHERE id = $1")
  Db.bind_int(h, 1, 2)
  check("second row", Db.step?(h))
  check_str("UTF-8 and backslash round-trip", "gädget ✓ \\ back", Db.column_text(h, 0))
  check("\"f\" reads false", !Db.column_bool(h, 1))
  check("\"f\" reads false when nullable", Db.column_bool_opt(h, 1) == false)
  check_int("bool false through column_int", 0, Db.column_int(h, 1))
  check("NULL float reads nil", Db.column_float_opt(h, 2).nil?)
  check("NULL float reads 0.0", Db.column_float(h, 2) == 0.0)
  check("NULL text reads nil", Db.column_text_opt(h, 3).nil?)
  check_str("NULL text reads empty", "", Db.column_text(h, 3))
  check("NULL int reads nil", Db.column_int_opt(h, 4).nil?)
  check_int("NULL int reads 0", 0, Db.column_int(h, 4))
  Db.finalize(h)

  h = Db.prepare("SELECT active FROM " + s + "widgets WHERE id = $1")
  Db.bind_int(h, 1, 3)
  check("third row", Db.step?(h))
  check("NULL bool reads nil", Db.column_bool_opt(h, 0).nil?)
  Db.finalize(h)
  nil
end

def case_04(s, schema)
  # Text and bool binds together.
  h = Db.prepare("SELECT id FROM " + s + "widgets WHERE name = $1 AND active = $2")
  Db.bind_text(h, 1, "gädget ✓ \\ back")
  Db.bind_bool(h, 2, false)
  check("text + bool binds", Db.step?(h))
  check_int("text + bool row", 2, Db.column_int(h, 0))
  Db.finalize(h)
  nil
end

def case_05(s, schema)
  # A nullable predicate renders IS NULL with no slot; the next value
  # keeps position $1.
  h = Db.prepare("SELECT id FROM " + s + "widgets WHERE gadget_id IS NULL AND name = $1")
  Db.bind_text(h, 1, "gädget ✓ \\ back")
  check("IS NULL keeps positions aligned", Db.step?(h))
  check_int("IS NULL row", 2, Db.column_int(h, 0))
  Db.finalize(h)

  # nil binds are SQL NULL, never the type's zero.
  h = Db.prepare("SELECT $1::int IS NULL, $2::text IS NULL, $3::boolean IS NULL, $4::int IS NULL, $5::boolean")
  Db.bind_int_opt(h, 1, nil)
  Db.bind_text_opt(h, 2, nil)
  Db.bind_bool_opt(h, 3, nil)
  Db.bind_int(h, 4, nil)
  Db.bind_bool(h, 5, true)
  check("nil binds row", Db.step?(h))
  check("bind_int_opt nil is NULL", Db.column_bool(h, 0))
  check("bind_text_opt nil is NULL", Db.column_bool(h, 1))
  check("bind_bool_opt nil is NULL", Db.column_bool(h, 2))
  check("bind_int nil is NULL", Db.column_bool(h, 3))
  check("bind_bool true is true", Db.column_bool(h, 4))
  Db.finalize(h)
  nil
end

def case_06(s, schema)
  # Several rows, in order, then done.
  h = Db.prepare_uncached("SELECT id FROM " + s + "widgets WHERE id >= $1 ORDER BY id")
  Db.bind_int(h, 1, 1)
  ids = []
  while Db.step?(h)
    ids.push(Db.column_int(h, 0))
  end
  Db.finalize(h)
  check_str("multi-row stepping", "[1, 2, 3]", ids.inspect)
  nil
end

def case_07(s, schema)
  # Zero rows.
  h = Db.prepare("SELECT id, name FROM " + s + "widgets WHERE id = $1")
  Db.bind_int(h, 1, 999)
  check("zero rows", !Db.step?(h))
  check_int("column_count with zero rows", 2, Db.column_count(h))
  check_raises("no current row", "no current row") { Db.column_int(h, 0) }
  Db.finalize(h)

  # column_value: the driver's native kinds, booleans as 1/0.
  h = Db.prepare("SELECT 42::bigint, 2.5::float8, 'x'::text, true, false, NULL::int")
  check("column_value row", Db.step?(h))
  check_str("column_value int", "42", Db.column_value(h, 0).inspect)
  check_str("column_value float", "2.5", Db.column_value(h, 1).inspect)
  check_str("column_value text", "\"x\"", Db.column_value(h, 2).inspect)
  check_str("column_value true", "1", Db.column_value(h, 3).inspect)
  check_str("column_value false", "0", Db.column_value(h, 4).inspect)
  check("column_value NULL", Db.column_value(h, 5).nil?)
  Db.finalize(h)
  nil
end

def case_08(s, schema)
  # ── Writes and their row counts ──

  Db.exec("UPDATE " + s + "widgets SET weight = 9.5 WHERE gadget_id = " + Db.escape_int(7))
  check_int("update changes", 2, Db.changes)
  Db.exec("UPDATE " + s + "widgets SET weight = 1 WHERE id = 999")
  check_int("update with no match", 0, Db.changes)
  h = Db.prepare("SELECT count(*) FROM " + s + "widgets WHERE weight = $1")
  Db.bind_text(h, 1, "9.5")
  check("updated rows visible", Db.step?(h))
  check_int("updated rows", 2, Db.column_int(h, 0))
  Db.finalize(h)
  Db.exec("DELETE FROM " + s + "widgets WHERE id = 3")
  check_int("delete changes", 1, Db.changes)
  nil
end

def case_09(s, schema)
  # ── Handle lifecycle ──

  conn = Db.current_conn
  base = conn.open_count
  h = Db.prepare("SELECT id FROM " + s + "widgets ORDER BY id")
  check("partial read", Db.step?(h))
  check_int("one handle open", base + 1, conn.open_count)
  Db.finalize(h)
  check_int("finalize mid-iteration releases", base, conn.open_count)
  check_raises("finalized handle refused", "unknown or finalized") { Db.step?(h) }
  h2 = Db.prepare("SELECT 1")
  check("a reused slot gets a new handle", h2 != h)
  check_raises("old handle still refused", "unknown or finalized") { Db.column_int(h, 0) }
  Db.finalize(h2)
  Db.finalize(h2)
  nil
end

def case_10(s, schema)
  # ── Leases ──

  check("no lease outside", !Db.in_lease?)
  # A connection that has drawn no sequence value: lastval has nothing,
  # so a uuid-key INSERT raises instead of answering a stale key.
  Db.with_connection do
    check("lease inside", Db.in_lease?)
    Db.exec("INSERT INTO " + s + "gadgets (label) VALUES ('uuid')")
    check_int("uuid insert changes", 1, Db.changes)
    check_raises("a fresh session with a uuid key", "no serial or identity primary key") { Db.last_insert_rowid }
  end
  leased = Db.current_conn
  Db.with_connection do
    leased = Db.current_conn
    h = Db.prepare("SELECT id FROM " + s + "widgets")
    Db.step?(h)
    check_int("handle open inside the lease", 1, leased.open_count)
  end
  check_int("lease end releases leftover handles", 0, leased.open_count)
  check_raises("the lease still releases on error", "widget failure") do
    Db.with_connection do
      leased = Db.current_conn
      Db.prepare("SELECT 1")
      raise "widget failure"
    end
  end
  check_int("released after the error", 0, leased.open_count)
  check("no lease after the error", !Db.in_lease?)
  nil
end

def case_11(s, schema)
  # ── Writes that return rows ──

  h = Db.exec_returning("INSERT INTO " + s + "widgets (name) VALUES ('ret-a'), ('ret-b') RETURNING id, name")
  check_int("returning changes", 2, Db.changes)
  check_int("returning column count", 2, Db.column_count(h))
  check("first returned row", Db.step?(h))
  first_id = Db.column_int(h, 0)
  check_str("first returned name", "ret-a", Db.column_text(h, 1))
  check("second returned row", Db.step?(h))
  check_int("returned ids ascend", first_id + 1, Db.column_int(h, 0))
  check("returned rows end", !Db.step?(h))
  Db.finalize(h)
  h = Db.exec_returning("INSERT INTO " + s + "gadgets (label) VALUES ('ret-uuid') RETURNING id")
  check("uuid key returned", Db.step?(h))
  check_int("uuid key as text", 36, Db.column_text(h, 0).length)
  Db.finalize(h)
  h = Db.exec_returning("UPDATE " + s + "widgets SET weight = 0 WHERE name LIKE 'ret-%' RETURNING id")
  check_int("update returning changes", 2, Db.changes)
  Db.finalize(h)
  h = Db.exec_returning("DELETE FROM " + s + "widgets WHERE id = -1 RETURNING id")
  check_int("no-match returning changes", 0, Db.changes)
  check("no-match returns no rows", !Db.step?(h))
  Db.finalize(h)
  Db.exec("DELETE FROM " + s + "widgets WHERE name LIKE 'ret-%'")
  nil
end

def case_12(s, schema)
  # ── SQLSTATE mapping ──

  Db.exec("CREATE TABLE " + s + "tags (id serial PRIMARY KEY, name varchar(3) NOT NULL UNIQUE)")
  Db.exec("INSERT INTO " + s + "tags (name) VALUES ('red')")
  dup = ""
  begin
    Db.exec("INSERT INTO " + s + "tags (name) VALUES ('red')")
  rescue ActiveRecord::RecordNotUnique => e
    dup = e.message
  end
  check("23505 is RecordNotUnique", dup.include?("duplicate key"))
  dup = ""
  begin
    h = Db.exec_returning("INSERT INTO " + s + "tags (name) VALUES ('red') RETURNING id")
  rescue ActiveRecord::RecordNotUnique => e
    dup = e.message
  end
  check("23505 from exec_returning", dup.include?("duplicate key"))
  long = ""
  begin
    Db.exec("INSERT INTO " + s + "tags (name) VALUES ('purple')")
  rescue ActiveRecord::ValueTooLong => e
    long = e.message
  end
  check("22001 is ValueTooLong", long.include?("too long"))
  h = Db.prepare("INSERT INTO " + s + "tags (name) VALUES ($1) RETURNING id")
  Db.bind_text(h, 1, "red")
  dup = ""
  begin
    Db.step?(h)
  rescue ActiveRecord::RecordNotUnique => e
    dup = e.message
  end
  Db.finalize(h)
  check("23505 from a bound statement", dup.include?("duplicate key"))
  check_str("unmapped codes stay PG::Error", "23502",
            sqlstate_of { Db.exec("INSERT INTO " + s + "tags (name) VALUES (NULL)") })
  nil
end

def case_13(s, schema)
  # ── Transactions ──

  Db.with_connection do
    Db.exec("BEGIN")
    Db.exec("INSERT INTO " + s + "tags (name) VALUES ('tmp')")
    Db.exec("ROLLBACK")
  end
  h = Db.prepare("SELECT count(*) FROM " + s + "tags WHERE name = 'tmp'")
  check("count row", Db.step?(h))
  check_int("a rolled-back transaction leaves no row", 0, Db.column_int(h, 0))
  Db.finalize(h)

  Db.with_connection do
    conn = Db.current_conn
    Db.exec("BEGIN")
    dup = ""
    begin
      Db.exec("INSERT INTO " + s + "tags (name) VALUES ('red')")
    rescue ActiveRecord::RecordNotUnique => e
      dup = e.message
    end
    check("duplicate inside a transaction", dup != "")
    check_int("failed transaction status", PG::PQTRANS_INERROR, conn.status)
    check_str("the next statement is refused", "25P02", sqlstate_of { Db.exec("SELECT 1") })
    Db.exec("ROLLBACK")
    check_int("idle after ROLLBACK", PG::PQTRANS_IDLE, conn.status)
    Db.exec("INSERT INTO " + s + "tags (name) VALUES ('blu')")
    check_int("usable after ROLLBACK", 1, Db.changes)
  end
  nil
end

def case_14(s, schema)
  # A lease that ends inside a transaction, or after a failure in one,
  # rolls back before the connection is reused.
  leased = Db.current_conn
  check_raises("abandoned transaction", "tag failure") do
    Db.with_connection do
      leased = Db.current_conn
      Db.exec("BEGIN")
      Db.exec("INSERT INTO " + s + "tags (name) VALUES ('grn')")
      raise "tag failure"
    end
  end
  check_int("abandoned lease returns idle", PG::PQTRANS_IDLE, leased.status)
  h = Db.prepare("SELECT count(*) FROM " + s + "tags WHERE name = 'grn'")
  check("abandoned count row", Db.step?(h))
  check_int("abandoned transaction rolled back", 0, Db.column_int(h, 0))
  Db.finalize(h)
  Db.with_connection do
    leased = Db.current_conn
    Db.exec("BEGIN")
    sqlstate_of { Db.exec("SELECT 1/0") }
  end
  check_int("failed transaction returns idle", PG::PQTRANS_IDLE, leased.status)
  nil
end

def case_15(s, schema)
  # A BEGIN outside a lease keeps its connection for a lease taken inside it.
  Db.exec("BEGIN")
  Db.with_connection do
    Db.exec("INSERT INTO " + s + "tags (name) VALUES ('yel')")
  end
  Db.exec("ROLLBACK")
  check("unpinned after ROLLBACK", !Db.in_lease?)
  h = Db.prepare("SELECT count(*) FROM " + s + "tags WHERE name = 'yel'")
  check("pinned count row", Db.step?(h))
  check_int("a lease inside a transaction joins it", 0, Db.column_int(h, 0))
  Db.finalize(h)
  nil
end

def case_16(s, schema)
  # A session the server ended is reopened on the next lease.
  pid = 0
  leased = Db.current_conn
  Db.with_connection do
    leased = Db.current_conn
    h = Db.prepare("SELECT pg_backend_pid()")
    Db.step?(h)
    pid = Db.column_int(h, 0)
    Db.finalize(h)
    Db.pool.first.exec("SELECT pg_terminate_backend(" + pid.to_s + ")")
    check_raises("terminated session", "connection") { Db.exec("SELECT 1") }
  end
  Db.with_connection do
    check("same connection slot", Db.current_conn == leased)
    h = Db.prepare("SELECT pg_backend_pid()")
    check("reopened session answers", Db.step?(h))
    check("a new backend", Db.column_int(h, 0) != pid)
    Db.finalize(h)
  end
  nil
end

def case_17(s, schema)
  # ── Server errors leave the connection usable ──

  failed = false
  begin
    Db.exec("SELEC 1")
  rescue PG::Error => e
    failed = e.message.include?("syntax error")
  end
  check("server error surfaces as PG::Error", failed)
  h = Db.prepare("SELECT $1::int + 1")
  Db.bind_int(h, 1, 41)
  check("usable after an error", Db.step?(h))
  check_int("usable after an error value", 42, Db.column_int(h, 0))
  Db.finalize(h)
  nil
end

def case_18(s, schema)
  # ── SQLite-only entry points ──

  check("read_snapshot_begin no-op", Db.read_snapshot_begin)
  check("read_snapshot_end no-op", Db.read_snapshot_end)
  check("checkpoint no-op", Db.checkpoint_in_background!.nil?)
  check("query_cache_begin no-op", Db.query_cache_begin.nil?)
  check("query_cache_end no-op", Db.query_cache_end.nil?)
  check_raises("seed_from_file refused", "pg_restore") { Db.seed_from_file("widgets.sqlite3") }
  nil
end

def case_19(s, schema)
  # ── Query capture ──

  log = Db.capture_sql do
    h = Db.prepare("SELECT 1")
    Db.finalize(h)
    Db.exec("SELECT 2")
  end
  check_str("capture_sql", "[\"SELECT 1\", \"SELECT 2\"]", log.inspect)
  check("sql_trace off", !Db.sql_trace?)
  nil
end

# Named statements this session holds on the server. Read through the
# unnamed statement, which pg_prepared_statements does not list.
def server_named
  h = Db.prepare_uncached("SELECT count(*) FROM pg_prepared_statements WHERE name LIKE 'rh_s%'")
  Db.step?(h)
  n = Db.column_int(h, 0)
  Db.finalize(h)
  n
end

def cached_read(s, id)
  h = Db.prepare("SELECT name FROM " + s + "widgets WHERE id = $1")
  Db.bind_int(h, 1, id)
  found = Db.step?(h)
  Db.finalize(h)
  found
end

def case_20(s, schema)
  # ── Named statements: reuse, and finalize keeps them ──

  Db.with_connection do
    conn = Db.current_conn
    base = server_named
    i = 0
    found = 0
    while i < 100
      found += 1 if cached_read(s, 1 + (i % 2))
      i += 1
    end
    check_int("100 cached reads", 100, found)
    check_int("one named statement for one shape", base + 1, server_named)
    check("the shape is cached", conn.named?("SELECT name FROM " + s + "widgets WHERE id = $1"))
    check_int("finalize released every handle", 0, conn.open_count)
    h = Db.prepare_uncached("SELECT name FROM " + s + "widgets WHERE id = $1 AND id > 0")
    Db.bind_int(h, 1, 1)
    check("uncached read", Db.step?(h))
    Db.finalize(h)
    check_int("an uncached read names nothing", base + 1, server_named)
  end
  nil
end

def case_21(s, schema)
  # ── Leases: handles never leak, statements stay cached ──

  leased = Db.current_conn
  before = 0
  Db.with_connection do
    leased = Db.current_conn
    before = server_named
    h = Db.prepare("SELECT id FROM " + s + "widgets WHERE id >= $1 ORDER BY id")
    Db.bind_int(h, 1, 1)
    Db.step?(h)
    check_int("open inside the lease", 1, leased.open_count)
  end
  check_int("no handle survives the lease", 0, leased.open_count)
  Db.with_connection do
    check("the same connection comes back", Db.current_conn == leased)
    check_int("its statement survived the lease", before + 1, server_named)
    h = Db.prepare("SELECT id FROM " + s + "widgets WHERE id >= $1 ORDER BY id")
    Db.bind_int(h, 1, 2)
    check("reused across leases", Db.step?(h))
    check_int("reuse parses nothing new", before + 1, server_named)
    Db.finalize(h)
  end

  # A second thread leases its own connection.
  Db.with_connection do
    mine = Db.current_conn
    other = Thread.new do
      same = true
      Db.with_connection do
        same = Db.current_conn == mine
      end
      same
    end
    check("another thread's lease is another connection", !other.value)
  end
  nil
end

def case_22(s, schema)
  # ── Eviction closes on the server ──

  Db.statement_cache_cap = 3
  Db.with_connection do
    conn = Db.current_conn
    i = 0
    while i < 5
      h = Db.prepare("SELECT " + i.to_s + " AS n, name FROM " + s + "widgets WHERE id = $1")
      Db.bind_int(h, 1, 1)
      Db.step?(h)
      Db.finalize(h)
      i += 1
    end
    check_int("the cache holds its bound", 3, conn.named_count)
    check_int("evicted statements are closed on the server", 3, server_named)
    check("the oldest shape was evicted", !conn.named?("SELECT 0 AS n, name FROM " + s + "widgets WHERE id = $1"))
    check("the newest shape stays", conn.named?("SELECT 4 AS n, name FROM " + s + "widgets WHERE id = $1"))
  end
  Db.statement_cache_cap = 128
  nil
end

def case_23(s, schema)
  # ── Recovery ──

  Db.with_connection do
    conn = Db.current_conn
    shape = "SELECT name FROM " + s + "widgets WHERE id = $1"
    check("warm", cached_read(s, 1))

    # Deallocated behind the cache's back: parsed again, outside a
    # transaction, without the caller seeing it.
    Db.exec("DEALLOCATE ALL")
    check("read after DEALLOCATE ALL", cached_read(s, 1))
    check_int("re-parsed once", 1, server_named)

    # An error inside a transaction: the named statement fails with the
    # transaction, a new shape is not recorded, and both work after
    # ROLLBACK, the old one without a new Parse.
    Db.exec("BEGIN")
    check("cached read in a transaction", cached_read(s, 2))
    check_str("an error fails the transaction", "22012", sqlstate_of { Db.exec("SELECT 1/0") })
    check_str("the named statement is refused", "25P02", sqlstate_of { cached_read(s, 1) })
    fresh = "SELECT id FROM " + s + "widgets WHERE name = $1"
    check_str("a new shape is refused", "25P02", sqlstate_of do
      h = Db.prepare(fresh)
      Db.bind_text(h, 1, "third")
      Db.step?(h)
    end)
    check("a refused Parse is not cached", !conn.named?(fresh))
    Db.exec("ROLLBACK")
    named = server_named
    check("reuse after ROLLBACK", cached_read(s, 1))
    check_int("no new Parse after ROLLBACK", named, server_named)
    h = Db.prepare(fresh)
    Db.bind_text(h, 1, "gädget ✓ \\ back")
    check("the refused shape parses after ROLLBACK", Db.step?(h))
    Db.finalize(h)

    # Deallocated inside a transaction: the error fails the transaction
    # and the entry is forgotten, so after ROLLBACK it parses again.
    Db.exec("BEGIN")
    Db.exec("DEALLOCATE ALL")
    check_str("a vanished statement inside a transaction", "26000", sqlstate_of { cached_read(s, 1) })
    check("forgotten", !conn.named?(shape))
    Db.exec("ROLLBACK")
    check("re-parsed after ROLLBACK", cached_read(s, 1))
    check("cached again", conn.named?(shape))
  end

  # A backend the server terminated takes its statements with it.
  leased = Db.current_conn
  pid = 0
  Db.with_connection do
    leased = Db.current_conn
    check("warm before terminate", cached_read(s, 1))
    check("named before terminate", leased.named_count > 0)
    h = Db.prepare_uncached("SELECT pg_backend_pid()")
    Db.step?(h)
    pid = Db.column_int(h, 0)
    Db.finalize(h)
    Db.pool.first.exec("SELECT pg_terminate_backend(" + pid.to_s + ")")
    check_raises("terminated mid-lease", "connection") { cached_read(s, 1) }
  end
  check_int("a dropped session forgets its statements", 0, leased.named_count)
  Db.with_connection do
    check("reopened read parses afresh", cached_read(s, 1))
    check_int("one statement on the new session", 1, server_named)
  end
  nil
end

def probe_name(s)
  h = Db.prepare_uncached("SELECT name FROM " + s + "txn_probe WHERE id = 1")
  Db.step?(h)
  v = Db.column_text(h, 0)
  Db.finalize(h)
  v
end

def case_24(s, schema)
  # ── #693 regression on Postgres: a flat transaction ──
  #
  # ActiveRecord::Base.transaction (connection.rb) reads Db._txn_depth
  # as its very FIRST statement, for every call — not only to decide
  # whether to join an already-open one. db_pg.rb never defined
  # Db._txn_depth / Db._txn_depth=, so ANY Model.transaction on the
  # PostgreSQL shim raised NoMethodError, flat or nested. Confirmed via
  # a bare `Db._txn_depth` call before this fix existed.
  Db.exec("CREATE TABLE " + s + "txn_probe (id integer PRIMARY KEY, name text NOT NULL)")
  Db.exec("INSERT INTO " + s + "txn_probe (id, name) VALUES (1, 'before')")
  check_int("depth starts at 0", 0, Db._txn_depth)

  ar_transaction { Db.exec("UPDATE " + s + "txn_probe SET name = 'committed' WHERE id = 1") }
  check_int("depth returns to 0 after a commit", 0, Db._txn_depth)
  check_str("a flat transaction commits", "committed", probe_name(s))

  raised = nil
  begin
    ar_transaction do
      Db.exec("UPDATE " + s + "txn_probe SET name = 'should-not-stick' WHERE id = 1")
      raise "boom"
    end
  rescue StandardError => e
    raised = e
  end
  check("a flat transaction's raise propagates", !raised.nil? && raised.message == "boom")
  check_int("depth returns to 0 after a rollback", 0, Db._txn_depth)
  check_str("a flat transaction rolls back on raise", "committed", probe_name(s))
  nil
end

def case_25(s, schema)
  # ── #693 regression on Postgres: a nested transaction joins ──
  #
  # A nested `transaction` sends no BEGIN of its own — it joins the
  # outer one (Rails' `requires_new: false` default) — which this
  # shim can only know by reading Db._txn_depth > 0. Depth going
  # 0 -> 1 -> 2 -> 1 -> 0 and a SINGLE Postgres transaction covering
  # both writes (one commit) is the observable proof of the join; a
  # bug that sent a second BEGIN would not raise (Postgres treats a
  # nested BEGIN as a no-op with a warning, not an error — so this is
  # the test that would catch it, not an error-based one).
  #
  # NOT covered here: an inner raise rolling back the outer write too.
  # Reproducing that (raise originating INSIDE the nested block, not
  # after it returns) surfaced a separate, pre-existing Spinel
  # exception-dispatch bug — confirmed on CRuby (correct) vs Spinel
  # (wrong: commits instead of rolling back) with the exact same
  # connection.rb bytes, and confirmed via the full real_blog pipeline
  # on SQLite too, so it is not a PostgreSQL or db_pg.rb issue. Flagged
  # separately rather than asserted on here or silently routed around.
  ar_transaction do
    check_int("depth is 1 in the outer block", 1, Db._txn_depth)
    Db.exec("UPDATE " + s + "txn_probe SET name = 'outer' WHERE id = 1")
    ar_transaction do
      check_int("depth is 2 in the joined nested block", 2, Db._txn_depth)
      Db.exec("UPDATE " + s + "txn_probe SET name = 'inner-committed' WHERE id = 1")
    end
    check_int("depth returns to 1 after the nested block returns", 1, Db._txn_depth)
  end
  check_int("depth returns to 0 after the outer block commits", 0, Db._txn_depth)
  check_str("nested and outer commit together", "inner-committed", probe_name(s))
  nil
end

def case_26(s, schema)
  # ── A COMMIT (or ROLLBACK) that itself raises still releases the pin ──
  #
  # Db.exec called pin_transaction only AFTER conn.exec(sql) returned.
  # If COMMIT or ROLLBACK itself raised — here, because the backend's
  # own session ended from under it — pin_transaction was skipped
  # entirely: the pin this thread took at BEGIN leaked, and
  # Db.current_conn stayed wedged on the dead connection for the rest
  # of the thread's life (roundhouse#693 fixed the same shape in
  # db.rb's Db.exec). A BEGIN outside a lease pins Db.pool.first (the
  # same connection Db.current_conn falls back to unleased), so
  # terminating ITS backend and then trying to COMMIT reproduces the
  # raise without ever taking a second lease on the pinned connection.
  #
  # `Db.with_connection` is re-entrant: once BEGIN has pinned this
  # thread, it would not lease a second connection at all, just hand
  # back the SAME pinned one (its own re-entrancy rule — see its
  # comment) — so sending the termination through it would kill the
  # connection out from under its OWN statement instead of the pinned
  # one. `Db.pool.conn(1)`, the shard's other connection, used directly
  # (as case_23 uses `Db.pool.first.exec` from inside a real lease), is
  # a genuinely separate session.
  Db.exec("BEGIN")
  check("BEGIN pins Db.pool.first", Db.current_conn.equal?(Db.pool.first) && Db.in_lease?)
  h = Db.prepare_uncached("SELECT pg_backend_pid()")
  Db.step?(h)
  pid = Db.column_int(h, 0)
  Db.finalize(h)
  Db.pool.conn(1).exec("SELECT pg_terminate_backend(" + pid.to_s + ")")
  raised = nil
  begin
    Db.exec("COMMIT")
  rescue StandardError => e
    raised = e
  end
  check("COMMIT on a terminated backend raises", !raised.nil?)
  check("the pin is released even though COMMIT raised", !Db.in_lease?)
  # pin_transaction also drops the dead connection on PQTRANS_UNKNOWN
  # (CodeRabbit, #766): Db.current_conn's unleased fallback is this
  # SAME Db.pool.first object, and PgConn#client only reopens when
  # @client is nil — releasing just the pin would leave every LATER
  # unleased Db.exec on this thread hitting the same dead socket
  # ("pg: connection lost") instead of reconnecting. No manual
  # Db.pool.first.drop here: that would hide a regression in the drop
  # above. A plain read on the unleased connection is the proof.
  check("an unleased read reconnects after the dead connection is dropped",
        cached_read(s, 1))
  nil
end

begin
  case_01(s, schema)
  case_02(s, schema)
  case_03(s, schema)
  case_04(s, schema)
  case_05(s, schema)
  case_06(s, schema)
  case_07(s, schema)
  case_08(s, schema)
  case_09(s, schema)
  case_10(s, schema)
  case_11(s, schema)
  case_12(s, schema)
  case_13(s, schema)
  case_14(s, schema)
  case_15(s, schema)
  case_16(s, schema)
  case_17(s, schema)
  case_18(s, schema)
  case_19(s, schema)
  case_20(s, schema)
  case_21(s, schema)
  case_22(s, schema)
  case_23(s, schema)
  case_24(s, schema)
  case_25(s, schema)
  case_26(s, schema)
ensure
  # Close every session first: a failed case can leave one holding locks
  # the DROP would otherwise wait on forever.
  Db.close
  Db.configure(url, pool_size: 1)
  Db.exec("DROP SCHEMA IF EXISTS " + schema + " CASCADE")
end

Db.close
check_raises("closed", "not configured") { Db.exec("SELECT 1") }
# A larger pool is sharded: four connections a shard.
Db.configure(url, pool_size: 8)
check_int("two shards for eight connections", 2, Db.shard_count)
Db.with_connection do
  h = Db.prepare_uncached("SELECT 1")
  check("a sharded lease reads", Db.step?(h))
  Db.finalize(h)
end
Db.close
puts "spinel_pg_db: " + $checks.to_s + " checks passed"
