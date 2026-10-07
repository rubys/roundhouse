# Every read below is in the *input* app (Parent), so the lowerer has to
# emit its SQL/binds; querying Relation directly in this consumer would only
# test the dynamic Ruby fallback. Exact results, not counts of successes.
i = 1
while i <= 32
  Db.exec("INSERT INTO parents (id, other_id, name) VALUES (" + i.to_s + ", 0, 'parent-" + i.to_s + "')")
  # Deliberately asymmetric: swapping (id, parent_id) must not find a
  # different row with the same COUNT(*). A reversal permutation hides it!
  Db.exec("INSERT INTO items (id, parent_id, name, required_flag) VALUES (" + i.to_s + ", " + ((i % 32) + 1).to_s + ", 'row-" + i.to_s + "', 0)")
  # Distinct group sizes make a count expose stale *existing* ids too.
  j = 0
  while j < i % 4
    extra_id = 1000 + i * 10 + j
    Db.exec("INSERT INTO items (id, parent_id, name, required_flag) VALUES (" + extra_id.to_s + ", " + i.to_s + ", 'extra-" + extra_id.to_s + "', 0)")
    j += 1
  end
  i += 1
end

probe = Parent.new
Db.with_connection do
  Db.query_cache_begin
  i = 0
  while i < 96
    id = ((i * 13) % 32) + 1
    raise "find id #{id}" unless probe.find_id(id) == id
    raise "find_by id #{id}" unless probe.find_by_id(33 - id) == 33 - id
    rows = probe.rows(id)
    raise "hydrate id #{id}" unless rows.length == 1 && rows[0].id == id && rows[0].name == "row-" + id.to_s
    raise "count hit #{id}" unless probe.count_id(id) == 1 + id % 4
    raise "count miss #{id}" unless probe.count_id(id + 100) == 0
    raise "exists hit #{id}" unless probe.exists_id(id)
    raise "exists miss #{id}" if probe.exists_id(id + 100)
    raise "two predicates #{id}" unless probe.pair(id, (id % 32) + 1) == 1
    raise "two predicates miss #{id}" unless probe.pair(id, id + 100) == 0
    raise "reload id #{id}" unless probe.reload_id(id) == "row-" + id.to_s
    children = probe.children(id)
    expected_ids = [((id + 30) % 32) + 1]
    j = 0
    while j < id % 4
      expected_ids.push(1000 + id * 10 + j)
      j += 1
    end
    raise "association id #{id}" unless children.map { |child| child.id }.sort == expected_ids
    raise "string id #{id}" unless probe.named("row-" + id.to_s) == id
    i += 1
  end
  Db.query_cache_end
end
puts "emit: 32 ids, 96 serial interleaved rounds, nine read methods passed"

# Nullable values must select NULL rows in either emit mode. The integer
# predicates on either side pin the order of the three optional bind slots.
Db.exec("INSERT INTO items (id, parent_id, name, number, optional_name, flag, required_flag) VALUES (501, 500, 'nullable', NULL, NULL, NULL, 0)")
Db.exec("INSERT INTO items (id, parent_id, name, number, optional_name, flag, required_flag) VALUES (502, 500, 'nullable', 0, '', 0, 0)")
Db.exec("INSERT INTO items (id, parent_id, name, number, optional_name, flag, required_flag) VALUES (503, 500, 'nullable', 73, '雪', 1, 0)")
Db.exec("INSERT INTO items (id, parent_id, name, required_flag) VALUES (0, 500, 'zero-key', 0)")
Db.exec("INSERT INTO items (id, parent_id, name, required_flag) VALUES (504, 0, 'zero-parent', 0)")
Db.exec("INSERT INTO items (id, parent_id, name, required_flag) VALUES (505, 500, '', 1)")
i = 0
while i < 12
  raise "nullable nil row" unless probe.nullable_pair(501, nil, nil, nil, 500) == 1
  raise "nullable zero row" unless probe.nullable_pair(502, 0, "", false, 500) == 1
  raise "nullable value row" unless probe.nullable_pair(503, 73, "雪", true, 500) == 1
  raise "nil must not match false/zero/empty" unless probe.nullable_pair(502, nil, nil, nil, 500) == 0
  raise "nullable final bind position" unless probe.nullable_pair(501, nil, nil, nil, 501) == 0
  raise "nullable RHS against nonnullable key" unless probe.nullable_key(nil) == 0
  raise "nil matched a NOT NULL zero value" unless probe.not_null_number(nil) == 0
  raise "NOT NULL zero remains a real value" unless probe.not_null_number(0) == 1
  raise "zero key is a real value" unless probe.nullable_key(0) == 1
  raise "nil matched a NOT NULL empty string" unless probe.not_null_name(nil) == 0
  raise "NOT NULL empty string remains a real value" unless probe.not_null_name("") == 1
  raise "nil matched a NOT NULL boolean" unless probe.not_null_flag(nil) == 0
  raise "NOT NULL false remains a real value" unless probe.not_null_flag(false) == 85
  raise "NOT NULL true remains a real value" unless probe.not_null_flag(true) == 1
  i += 1
end
puts "emit: nullable int/text/bool and surrounding bind positions passed"

# Three independently nullable predicates have eight SQL shapes. Visit
# every combination repeatedly with a different id and a fixed trailing
# predicate; a skipped NULL slot must not shift any later bind incorrectly.
mask = 0
while mask < 8
  number_sql = (mask & 1) == 0 ? "NULL" : "73"
  name_sql = (mask & 2) == 0 ? "NULL" : "'snow'"
  flag_sql = (mask & 4) == 0 ? "NULL" : "1"
  Db.exec("INSERT INTO items (id, parent_id, name, number, optional_name, flag, required_flag) VALUES (" +
          (600 + mask).to_s + ", 900, 'combination', " + number_sql + ", " + name_sql + ", " + flag_sql + ", 0)")
  mask += 1
end
12.times do
  mask = 0
  while mask < 8
    number = (mask & 1) == 0 ? nil : 73
    name = (mask & 2) == 0 ? nil : "snow"
    flag = (mask & 4) == 0 ? nil : true
    raise "nullable combination #{mask}" unless probe.nullable_pair(600 + mask, number, name, flag, 900) == 1
    raise "nullable combination trailing bind #{mask}" unless probe.nullable_pair(600 + mask, number, name, flag, 901) == 0
    mask += 1
  end
end
puts "emit: all eight nullable SQL shapes, 192 interleaved hit/miss reads passed"

# Public key methods receive nil even though their synthesized primitive is
# key-typed. Rails rejects it before touching SQLite; id=0 must not match.
raise "exists?(nil) matched zero key" if probe.nil_key_exists
begin
  probe.nil_key_find
  raise "find(nil) matched zero key"
rescue ActiveRecord::RecordNotFound
end
puts "emit: find(nil) and exists?(nil) reject a real zero key passed"
