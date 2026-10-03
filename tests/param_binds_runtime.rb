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
# Nesting an identical shape is not covered yet; that needs a separate fix.
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

# Observe bytes as a BLOB, not SQLite length(TEXT) or column_text's C-string
# conversion: SQLite string expressions on embedded NUL are not specified.
# The bound value must preserve all three bytes (61 00 62).
stmt = Db.prepare("SELECT hex(CAST(? AS BLOB)) AS nul_bytes")
Db.bind_text(stmt, 1, "a\0b")
raise "missing NUL text" if !Db.step?(stmt)
expect_text("embedded NUL bytes", "610062", Db.column_text(stmt, 0))
Db.finalize(stmt)
puts "runtime: embedded NUL bytes passed"
