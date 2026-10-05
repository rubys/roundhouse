# Every read below is in the *input* app (Parent), so the lowerer has to
# emit its SQL/binds; querying Relation directly in this consumer would only
# test the dynamic Ruby fallback. Exact results, not counts of successes.
i = 1
while i <= 32
  Db.exec("INSERT INTO parents (id, other_id, name) VALUES (" + i.to_s + ", 0, 'parent-" + i.to_s + "')")
  # Deliberately asymmetric: swapping (id, parent_id) must not find a
  # different row with the same COUNT(*). A reversal permutation hides it!
  Db.exec("INSERT INTO items (id, parent_id, name) VALUES (" + i.to_s + ", " + ((i % 32) + 1).to_s + ", 'row-" + i.to_s + "')")
  # Distinct group sizes make a count expose stale *existing* ids too.
  j = 0
  while j < i % 4
    extra_id = 1000 + i * 10 + j
    Db.exec("INSERT INTO items (id, parent_id, name) VALUES (" + extra_id.to_s + ", " + i.to_s + ", 'extra-" + extra_id.to_s + "')")
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
