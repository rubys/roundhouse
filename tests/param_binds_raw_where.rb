# Execute raw Relation predicates against real rows. Escaped values must
# never become the input to a later placeholder or replacement expansion.
Db.with_connection do
  backslashes = %q{path\1\&\`\'} + "雪"
  Db.exec("INSERT INTO items (id, parent_id, name, required_flag) VALUES (1, 1, 'a', 0), (2, 1, 'what?', 0)")
  Db.exec("INSERT INTO items (id, parent_id, name, required_flag) VALUES (3, 2, " + Db.escape_string(backslashes) + ", 0)")

  scalar = ActiveRecord::Relation.new(Item).where("items.name = ? OR items.id = ?", "what?", 1)
  raise "scalar value consumed later placeholder" unless scalar.count == 2
  escaped = ActiveRecord::Relation.new(Item).where("items.name = ? AND items.id = ?", backslashes, 3)
  raise "replacement escapes changed scalar value" unless escaped.count == 1
  quoted = ActiveRecord::Relation.new(Item).where("items.name = ? OR items.id = ?", "x' OR ?=1 --", 999)
  raise "quoted scalar changed predicate" unless quoted.count == 0
  reversed = ActiveRecord::Relation.new(Item).where("items.id = ? AND items.name = ?", 2, "what?")
  raise "last argument changed" unless reversed.count == 1
  negated = ActiveRecord::Relation.new(Item).not("items.name = ? OR items.id = ?", "what?", 1)
  raise "negated substitution changed" unless negated.count == 1
  having = ActiveRecord::Relation.new(Item).group(:id).having("items.name = ? AND items.id = ?", "what?", 2)
  raise "HAVING substitution changed" unless having.load_records.length == 1

  # Keep main's adapter treatment of Arrays, not the draft's IN-list expansion.
  list = ActiveRecord::Relation.new(Item).where("items.id IN (?) AND items.parent_id = ?", [1, 2], 1)
  raise "Array escaping changed" unless list.to_sql.include?("items.id IN ('[1, 2]') AND items.parent_id = 1")
  empty = ActiveRecord::Relation.new(Item).where("items.id IN (?)", [])
  raise "empty Array escaping changed" unless empty.to_sql.include?("items.id IN ('[]')")
  hashed = ActiveRecord::Relation.new(Item).where(id: [1, 3])
  raise "hash IN changed" unless hashed.count == 2

  # This repair does not add a SQL parser or an arity policy.
  literal = ActiveRecord::Relation.new(Item).where("items.id = 1", "ignored?")
  raise "literal raw clause changed" unless literal.count == 1
  surplus = ActiveRecord::Relation.new(Item).where("items.name = ?", "what?", "ignored")
  raise "surplus argument rewrote substituted data" unless surplus.count == 1
  missing = ActiveRecord::Relation.new(Item).where("items.name = ? AND items.id = ?", "what?")
  raise "missing argument changed placeholder" unless missing.to_sql.include?("items.name = 'what?' AND items.id = ?")
  none = ActiveRecord::Relation.new(Item).where("items.id = ?")
  raise "unfilled trailing placeholder changed" unless none.to_sql.include?("items.id = ?")
  adjacent = ActiveRecord::Relation.new(Item).where("??", "a", "b")
  raise "adjacent placeholders changed" unless adjacent.to_sql.include?("('a''b')")
  blank = ActiveRecord::Relation.new(Item).where("", "ignored")
  raise "empty fragment changed" if blank.to_sql.include?("WHERE")
end
puts "raw where: scalar substitution, escaping, negation, HAVING and Array compatibility passed"
