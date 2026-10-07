# Contract item 8: nil against a NOT NULL column must never select zero.
# Use the typed primitive directly, as well as the emitted optional path.
Db.exec("CREATE TABLE bind_not_null (value INTEGER NOT NULL)")
Db.exec("INSERT INTO bind_not_null VALUES (0), (73)")
def check_not_null_integer(value)
  stmt = Db.prepare("SELECT COUNT(*) FROM bind_not_null WHERE value = ?")
  Db.bind_int(stmt, 1, value)
  raise "missing NOT NULL count" unless Db.step?(stmt)
  expected = value.nil? ? 0 : 1
  actual = Db.column_int(stmt, 0)
  Db.finalize(stmt)
  raise "typed bind_int(nil) matched zero" unless actual == expected
end
12.times do
  check_not_null_integer(73)
  check_not_null_integer(nil)
  check_not_null_integer(0)
  check_not_null_integer(nil)
end
puts "runtime: 48 typed integer/NULL comparisons against NOT NULL passed"
