# The primitive contract underneath emitted readers. Run against the real Db
# shim, not a mock. The Rust harness supplies Db and a shared database.
def expect_int(label, expected, actual)
  raise label + ": expected " + expected.to_s + ", got " + actual.to_s if expected != actual
end

def expect_text(label, expected, actual)
  if expected != actual
    raise label + ": expected hex " + expected.unpack1("H*") + ", got hex " + actual.unpack1("H*")
  end
end

Db.exec("CREATE TABLE bind_rows (id INTEGER PRIMARY KEY, owner_id INTEGER NOT NULL, label TEXT NOT NULL)")
i = 1
while i <= 32
  Db.exec("INSERT INTO bind_rows VALUES (" + i.to_s + ", " + (1000 + i).to_s + ", 'row-" + i.to_s + "')")
  i += 1
end

def read_bound_id(id)
  stmt = Db.prepare("SELECT id, label FROM bind_rows WHERE id = ?")
  Db.bind_int(stmt, 1, id)
  raise "missing id " + id.to_s if !Db.step?(stmt)
  expect_int("varying id", id, Db.column_int(stmt, 0))
  expect_text("varying label", "row-" + id.to_s, Db.column_text(stmt, 1))
  # Point readers finalize without stepping to DONE. Skipping reset here
  # cannot be masked by SQLite's automatic reset after DONE.
  Db.finalize(stmt)
end

# A live outer cursor survives reuse of other shapes on the SAME connection.
# Identical-shape ownership is covered by StatementCacheTest above.
Db.with_connection do
  Db.query_cache_begin
  outer = Db.prepare("SELECT id FROM bind_rows WHERE id >= ? ORDER BY id")
  Db.bind_int(outer, 1, 1)
  i = 1
  while i <= 32
    raise "outer cursor ended early" if !Db.step?(outer)
    expect_int("outer cursor", i, Db.column_int(outer, 0))
    id = ((i * 13) % 32) + 1
    read_bound_id(id)
    count = Db.prepare("SELECT COUNT(*) FROM bind_rows WHERE id <= ?")
    Db.bind_int(count, 1, id)
    raise "missing count" if !Db.step?(count)
    expect_int("varying count", id, Db.column_int(count, 0))
    Db.finalize(count)
    read_bound_id(33 - id)
    pair = Db.prepare("SELECT id FROM bind_rows WHERE id = ? AND owner_id = ?")
    Db.bind_int(pair, 1, id)
    Db.bind_int(pair, 2, 1000 + id)
    raise "two-bind query missed" if !Db.step?(pair)
    expect_int("two-bind query", id, Db.column_int(pair, 0))
    Db.finalize(pair)
    i += 1
  end
  raise "outer cursor has extra rows" if Db.step?(outer)
  Db.finalize(outer)
  Db.query_cache_end
end
puts "runtime: varying ids, interleaved live cursors and request cache passed"

# A barrier guarantees four leases are live simultaneously, with cold
# prepares on the remaining pool connections. Thread#value propagates failures.
ready = Queue.new
go = Queue.new
threads = []
4.times do |worker|
  threads.push(Thread.new(worker) do |number|
    Db.with_connection do
      Db.query_cache_begin
      # All four statements are bound before ANY of them steps. A cache
      # accidentally shared between connections cannot win a timing lottery.
      held = Db.prepare("SELECT id, label FROM bind_rows WHERE id = ?")
      Db.bind_int(held, 1, number + 1)
      ready.push(1)
      go.pop
      raise "missing concurrent row" if !Db.step?(held)
      expect_int("concurrent bound id", number + 1, Db.column_int(held, 0))
      Db.finalize(held)
      i = 0
      while i < 64
        read_bound_id(((i * 13 + number * 7) % 32) + 1)
        Thread.pass
        i += 1
      end
      Db.query_cache_end
    end
    64
  end)
end
4.times { ready.pop }
4.times { go.push(1) }
threads.each { |thread| expect_int("thread completed", 64, thread.value) }
puts "runtime: four simultaneous bound statements, 260 checked reads passed"

def roundtrip_text(value)
  stmt = Db.prepare("SELECT ? AS text_value")
  Db.bind_text(stmt, 1, value)
  raise "missing text" if !Db.step?(stmt)
  expect_text("text roundtrip", value, Db.column_text(stmt, 0))
  Db.finalize(stmt)
end
roundtrip_text("quote's \"double\" ? -- SQL")
roundtrip_text("雪 café 🦀")
roundtrip_text("long-" + "é雪" * 8192)

# SQLITE_TRANSIENT means a snapshot at bind time. Mutation before step is a
# deterministic discriminator for SQLITE_STATIC; allocator reuse after GC
# alone would make a flaky negative control.
text = "mutable-" + 123.to_s
stmt = Db.prepare("SELECT ? AS copied_text")
Db.bind_text(stmt, 1, text)
text.setbyte(0, 88)
raise "missing copied text" if !Db.step?(stmt)
expect_text("bind_text must copy before returning", "mutable-123", Db.column_text(stmt, 0))
Db.finalize(stmt)

def bind_ephemeral(stmt)
  # The caller retains neither the object nor a reference to its buffer.
  Db.bind_text(stmt, 1, "ephemeral-" + "雪é" * 2048)
end
stmt = Db.prepare("SELECT ? AS collected_text")
bind_ephemeral(stmt)
GC.start
32.times { |n| garbage = n.to_s + "xxxxx" * 2048 }
GC.start
raise "missing collected text" if !Db.step?(stmt)
expect_text("collected bind_text", "ephemeral-" + "雪é" * 2048, Db.column_text(stmt, 0))
Db.finalize(stmt)
puts "runtime: quotes, UTF-8, long text, copy ownership and GC passed"

# Reads must match the values Db.exec writes inline, not merely round-trip
# through the bind API. In particular SQLite TEXT and BLOB with identical
# bytes are not equal, and ASCII-only BINARY strings are written as TEXT.
Db.exec("CREATE TABLE bind_string_rows (value TEXT NOT NULL)")
def inline_bound_string(label, value)
  Db.exec("DELETE FROM bind_string_rows")
  Db.exec("INSERT INTO bind_string_rows VALUES (" + Db.escape_string(value) + ")")
  stored = Db.prepare("SELECT typeof(value), hex(value) FROM bind_string_rows")
  raise "missing inline string" if !Db.step?(stored)
  storage_type = Db.column_text(stored, 0)
  storage_bytes = Db.column_text(stored, 1)
  Db.finalize(stored)

  inline = Db.prepare("SELECT COUNT(*) FROM bind_string_rows WHERE value = " + Db.escape_string(value))
  raise "missing inline count" if !Db.step?(inline)
  expect_int(label + " inline lookup", 1, Db.column_int(inline, 0))
  Db.finalize(inline)
  bound = Db.prepare("SELECT COUNT(*) FROM bind_string_rows WHERE value = ?")
  Db.bind_text(bound, 1, value)
  raise "missing bound count" if !Db.step?(bound)
  expect_int(label + " bound lookup", 1, Db.column_int(bound, 0))
  Db.finalize(bound)

  bytes = Db.prepare("SELECT typeof(?), hex(?)")
  Db.bind_text(bytes, 1, value)
  Db.bind_text(bytes, 2, value)
  raise "missing bound bytes" if !Db.step?(bytes)
  expect_text(label + " storage class", storage_type, Db.column_text(bytes, 0))
  expect_text(label + " stored bytes", storage_bytes, Db.column_text(bytes, 1))
  Db.finalize(bytes)
end
inline_bound_string("quotes", "quote's \"double\" ? -- SQL")
inline_bound_string("UTF-8", "雪 café 🦀")
inline_bound_string("ASCII binary", "plain-ascii".b)
inline_bound_string("empty binary", "".b)
inline_bound_string("UTF-8 binary", "café".b)
inline_bound_string("invalid UTF-8 binary", "\xFF\xFE'".b)
puts "runtime: inline writes and bound reads agree on text/binary storage and bytes"

# Observe bytes as a BLOB, not SQLite length(TEXT) or column_text's C-string
# conversion: SQLite string expressions on embedded NUL are not specified.
# The bound value must preserve all three bytes (61 00 62).
stmt = Db.prepare("SELECT hex(CAST(? AS BLOB)) AS nul_bytes")
Db.bind_text(stmt, 1, "a\0b")
raise "missing NUL text" if !Db.step?(stmt)
expect_text("embedded NUL bytes", "610062", Db.column_text(stmt, 0))
Db.finalize(stmt)
puts "runtime: embedded NUL bytes passed"

# The primitive must keep nil distinct from false even if a caller bypasses
# the lowerer's nullable inline path. Alternate on one cached shape as well.
def expect_bound_bool(value, expected)
  stmt = Db.prepare("SELECT COALESCE(?, -7)")
  Db.bind_bool(stmt, 1, value)
  raise "missing bool row" if !Db.step?(stmt)
  expect_int("nullable bool", expected, Db.column_int(stmt, 0))
  Db.finalize(stmt)
end

expect_bound_bool(false, 0)
expect_bound_bool(nil, -7)
expect_bound_bool(true, 1)
expect_bound_bool(nil, -7)
expect_bound_bool(false, 0)
puts "runtime: nullable boolean preserves SQL NULL passed"

# A write that returns rows: the handle replays them, `changes` is the
# write's count, and the write empties the request's query cache.
# RETURNING needs SQLite 3.35; an older library raises a clear error.
raise "3.35.0 supports RETURNING" if !Db.returning_supported?(3035000)
raise "3.34.1 has no RETURNING" if Db.returning_supported?(3034001)
Db.exec("CREATE TABLE returning_rows (id INTEGER PRIMARY KEY, label TEXT NOT NULL UNIQUE)")
h = Db.exec_returning("INSERT INTO returning_rows (label) VALUES ('alpha'), ('beta') RETURNING id, label")
expect_int("returning changes", 2, Db.changes)
expect_int("returning column count", 2, Db.column_count(h))
# SQLite does not promise an order for RETURNING rows: read both, then
# accept either order.
raise "missing first returned row" if !Db.step?(h)
id_a = Db.column_int(h, 0)
label_a = Db.column_text(h, 1)
raise "missing second returned row" if !Db.step?(h)
id_b = Db.column_int(h, 0)
label_b = Db.column_text(h, 1)
raise "extra returned row" if Db.step?(h)
in_order = id_a == 1 && label_a == "alpha" && id_b == 2 && label_b == "beta"
reversed = id_a == 2 && label_a == "beta" && id_b == 1 && label_b == "alpha"
if !in_order && !reversed
  raise "returned rows: expected {1 alpha, 2 beta} in any order, got " +
    id_a.to_s + " " + label_a + ", " + id_b.to_s + " " + label_b
end
Db.finalize(h)
h = Db.exec_returning("UPDATE returning_rows SET label = 'none' WHERE id = 99 RETURNING id")
expect_int("returning no rows changes", 0, Db.changes)
raise "returned a row for no match" if Db.step?(h)
Db.finalize(h)
Db.with_connection do
  Db.query_cache_begin
  stmt = Db.prepare("SELECT COUNT(*) FROM returning_rows")
  raise "missing count" if !Db.step?(stmt)
  expect_int("count before", 2, Db.column_int(stmt, 0))
  Db.finalize(stmt)
  h = Db.exec_returning("INSERT INTO returning_rows (label) VALUES ('gamma') RETURNING id")
  raise "missing gamma id" if !Db.step?(h)
  expect_int("gamma id", 3, Db.column_int(h, 0))
  Db.finalize(h)
  stmt = Db.prepare("SELECT COUNT(*) FROM returning_rows")
  raise "missing count after" if !Db.step?(stmt)
  expect_int("exec_returning clears the query cache", 3, Db.column_int(stmt, 0))
  Db.finalize(stmt)
  Db.query_cache_end
end
puts "runtime: exec_returning rows, row count and cache invalidation passed"

# exec_returning's capture needs releasing like any other reader. On the
# Spinel SQLite shim (runtime/spinel/db.rb) that capture lives in
# @qc_cursors, an array only cleared in bulk at a lease boundary
# (query_cache_begin/end); outside with_connection there is no lease at
# all, so a script that calls exec_returning then finalize — with
# nothing else ever touching the query cache — must not grow that array
# without bound. Db.qc_cursor_count is a test-only hook: it is always 0
# on the CRuby/JRuby shims, which hold no such array — a handle there is
# a plain Ruby object, reclaimed by GC once finalize drops the last
# reference to it.
before = Db.qc_cursor_count
i = 0
while i < 1000
  h = Db.exec_returning("INSERT INTO returning_rows (label) VALUES ('loop-" + i.to_s + "') RETURNING id")
  raise "missing loop row" if !Db.step?(h)
  Db.finalize(h)
  i += 1
end
expect_int("exec_returning does not leak captures outside a lease", before, Db.qc_cursor_count)
puts "runtime: exec_returning outside with_connection does not leak captures passed"

# A UNIQUE violation while stepping exec_returning is RecordNotUnique,
# and statement cleanup must not replace it: the failed step's reset has
# already consumed the error, so the finalize in the ensure reports OK.
# The connection then serves a plain read.
def expect_returning_not_unique(label)
  h = Db.exec_returning("INSERT INTO returning_rows (label) VALUES ('" + label + "') RETURNING id")
  raise "missing " + label + " id" if !Db.step?(h)
  Db.finalize(h)
  raised = "nothing"
  begin
    h = Db.exec_returning("INSERT INTO returning_rows (label) VALUES ('" + label + "') RETURNING id")
    Db.finalize(h)
  rescue ActiveRecord::RecordNotUnique
    raised = "RecordNotUnique"
  rescue RuntimeError => e
    raised = "RuntimeError: " + e.message
  end
  expect_text("exec_returning duplicate " + label, "RecordNotUnique", raised)
  stmt = Db.prepare("SELECT COUNT(*) FROM returning_rows WHERE label = '" + label + "'")
  raise "missing count after duplicate " + label if !Db.step?(stmt)
  expect_int("rows after duplicate " + label, 1, Db.column_int(stmt, 0))
  Db.finalize(stmt)
end
expect_returning_not_unique("dup-outside")
Db.with_connection do
  expect_returning_not_unique("dup-inside")
end
puts "runtime: exec_returning UNIQUE violation raises RecordNotUnique passed"
