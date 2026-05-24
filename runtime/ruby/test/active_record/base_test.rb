require_relative "../test_helper"

# Skip entirely when `Db` / `SqliteAdapter` aren't loaded — the requires
# in test_helper.rb tolerate environments without the sqlite3 gem or
# without the runtime/spinel/ subtree (per-target scratch in
# framework_tests_ruby.rs). Defining BaseTest at all would surface
# `NameError: uninitialized constant Db` at setup time.
unless defined?(Db) && defined?(SqliteAdapter)
  warn "skipping BaseTest: Db / SqliteAdapter not loaded (sqlite3 gem unavailable or spinel/ not on load path)"
  return
end

# Direct unit tests for `runtime/ruby/active_record/base.rb`. Each
# CRUD method (find/all/where/count/exists?/save/destroy/reload/
# create/create!/last) gets exercised against an in-memory SQLite
# database routed through `Db` + `SqliteAdapter` (the CRuby gem-backed
# variant — see runtime/spinel/db_cruby.rb required by test_helper).
# A subclass defines the per-model overrides Base requires
# (`table_name`, `schema_columns`, `instantiate`, `attributes`,
# `assign_from_row`) so the framework dispatch + lifecycle is what's
# under test, not the synthesized model code.
#
# Current state: runs under CRuby via the `framework_ruby_tests_pass`
# autorun gate. Per-target runner functions in
# tests/framework_tests_{typescript,crystal,spinel}.rs are DISABLED
# pending a follow-on session that will wire each target against its
# native sqlite adapter (spinel: libsqlite3 FFI; Crystal: DB::SQLite3;
# TS: better-sqlite3 / libsql; Rust: rusqlite; Go: modernc.org/sqlite).
# The prior `FrameworkTestAdapter` polymorphic-Hash mock plus its 5
# per-target mirror files have been removed; this test going forward
# travels through the same real-sqlite path as production.
class BaseTest < Minitest::Test
  # Minimal user-facing model — just enough to satisfy Base's
  # contract markers. Stores a small (id, title) row shape;
  # `attributes` returns the hash the adapter writes.
  class Item < ActiveRecord::Base
    attr_accessor :title

    def self.table_name = "items"
    def self.schema_columns = [:id, :title]

    def self.instantiate(row)
      it = new
      # Two cross-target patterns at play here:
      #   1. String-keyed row access (`row["id"]`, not `row[:id]`).
      #      Production sqlite adapters return `Hash<String, ...>`;
      #      FrameworkTestAdapter (both Ruby and Crystal) matches
      #      that key convention so test fixtures don't need to
      #      branch on adapter shape.
      #   2. Cell narrowing for typed setters. Crystal sees the cell
      #      value as a wide union (DB::Any | TestCellValue); strict
      #      `id`/`title` setters reject directly:
      #        - `.to_s.to_i` — Ruby Integer + Crystal Int32 (auto-
      #          coerced to Int64 at the typed setter call).
      #        - bind `row["k"]` to a local var, then
      #          `setter = local if local.is_a?(T)` — Crystal
      #          narrows simple-variable guards even in postfix-if
      #          form, but doesn't narrow arbitrary expressions
      #          like `row["k"]` directly.
      it.id = row["id"].to_s.to_i
      title = row["title"]
      it.title = title if title.is_a?(String)
      it.mark_persisted!()
      it
    end

    def attributes
      # `.to_h` is a no-op on Ruby Hash and a NamedTuple→Hash
      # conversion under Crystal's strict typing. The downstream
      # `ActiveRecord.adapter.insert(table, attributes)` slot
      # is typed `Hash(Symbol, _)`; without the conversion
      # Crystal sees the literal as NamedTuple and rejects.
      { title: @title }.to_h
    end

    def assign_from_row(row)
      title = row["title"]
      @title = title if title.is_a?(String)
    end

    # Legacy 12-method-shim routing. Production models override
    # `_adapter_insert`/etc. directly with per-table `Db.exec` calls
    # (Level-3 emit); Item is the only hand-written subclass still
    # exercising the `ActiveRecord.adapter.*` path. Explicit overrides
    # let Base's primitives stay empty for spinel polymorphic dispatch
    # (see runtime/ruby/active_record/base.rb). Static `Item.table_name`
    # avoids the test-lowerer's `self.class` → `class` self-stripping
    # corner case; explicit `attributes()` because the TS emitter
    # currently drops parens from bare-name method calls (emits as
    # method reference instead of invocation).
    def _adapter_insert
      ActiveRecord.adapter.insert(Item.table_name, attributes())
    end

    def _adapter_update
      ActiveRecord.adapter.update(Item.table_name, @id, attributes())
    end

    def _adapter_delete
      ActiveRecord.adapter.delete(Item.table_name, @id)
    end

    def _adapter_reload
      row = ActiveRecord.adapter.find(Item.table_name, @id)
      return nil if row.nil?
      assign_from_row(row)
      self
    end
  end

  def setup
    Db.configure(":memory:")
    Db.exec("CREATE TABLE items (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT)")
    ActiveRecord.adapter = SqliteAdapter
  end

  def teardown
    Db.close
  end

  # ── persistence-state predicates ────────────────────────────

  def test_new_record_starts_unpersisted
    item = Item.new
    assert_predicate item, :new_record?
    refute_predicate item, :persisted?
    refute_predicate item, :destroyed?
  end

  def test_save_marks_persisted
    item = Item.new
    item.title = "Hi"
    assert item.save()
    assert_predicate item, :persisted?
    refute_predicate item, :new_record?
  end

  def test_save_assigns_id
    item = Item.new
    item.title = "Hi"
    item.save()
    refute_equal 0, item.id
  end

  # ── class-level finders ─────────────────────────────────────

  def test_find_returns_instance_for_existing_row
    a = Item.new; a.title = "A"; a.save()
    found = Item.find(a.id)
    assert_kind_of Item, found
    assert_equal "A", found.title
    assert_predicate found, :persisted?
  end

  def test_find_raises_record_not_found_when_missing
    err = assert_raises(ActiveRecord::RecordNotFound) { Item.find(999) }
    assert_match(/id=999/, err.message)
  end

  def test_find_by_returns_first_match_or_nil
    a = Item.new; a.title = "A"; a.save()
    b = Item.new; b.title = "B"; b.save()

    found = Item.find_by(title: "B")
    # `raise if nil?` is the cross-target nullable narrowing idiom:
    # Ruby raises with a clear message instead of crashing on
    # `nil.id` later; Crystal narrows `found` to non-nil for the
    # subsequent access. `found.try(&.id)` would also work but
    # silently passes the assertion when the result IS nil.
    raise "expected find_by to return non-nil" if found.nil?
    assert_equal b.id, found.id

    miss = Item.find_by(title: "Nope")
    assert_nil miss
  end

  def test_where_filters_to_matching_rows
    a = Item.new; a.title = "A"; a.save()
    b = Item.new; b.title = "B"; b.save()
    c = Item.new; c.title = "A"; c.save()

    matches = Item.where(title: "A")
    assert_equal 2, matches.length
    assert(matches.all? { |m| m.title == "A" })
  end

  def test_all_returns_every_row
    3.times { |i| it = Item.new; it.title = "T#{i}"; it.save() }
    assert_equal 3, Item.all.length
  end

  def test_count_matches_all_size
    2.times { |i| it = Item.new; it.title = "T#{i}"; it.save() }
    assert_equal 2, Item.count
  end

  def test_exists_returns_true_for_present_id_false_for_absent
    a = Item.new; a.title = "A"; a.save()
    assert Item.exists?(a.id)
    refute Item.exists?(a.id + 9999)
  end

  def test_last_returns_highest_id_or_nil_when_empty
    assert_nil Item.last
    a = Item.new; a.title = "A"; a.save()
    b = Item.new; b.title = "B"; b.save()
    last = Item.last
    raise "expected last to return non-nil after save" if last.nil?
    assert_equal b.id, last.id
  end

  # ── update + destroy ────────────────────────────────────────

  def test_save_updates_existing_record
    a = Item.new; a.title = "Original"; a.save()
    fetched = Item.find(a.id)
    fetched.title = "Updated"
    fetched.save()
    refetched = Item.find(a.id)
    assert_equal "Updated", refetched.title
  end

  def test_destroy_removes_row_and_marks_destroyed
    a = Item.new; a.title = "Doomed"; a.save()
    a.destroy()
    assert_predicate a, :destroyed?
    assert_raises(ActiveRecord::RecordNotFound) { Item.find(a.id) }
  end

  def test_destroy_on_unpersisted_returns_self_without_touching_db
    item = Item.new
    item.destroy()
    refute_predicate item, :destroyed?
  end

  def test_reload_refreshes_from_db
    a = Item.new; a.title = "First"; a.save()
    # Mutate via a separate fetch to simulate concurrent change.
    other = Item.find(a.id)
    other.title = "Second"
    other.save()

    a.reload()
    assert_equal "Second", a.title
  end

  def test_destroy_all_removes_every_row
    3.times { |i| it = Item.new; it.title = "T#{i}"; it.save() }
    Item.destroy_all
    assert_equal 0, Item.count
  end

  # ── create / create! factories ──────────────────────────────
  #
  # Base.create / Base.create! use `new(attrs)` followed by `save`.
  # The default Base#initialize ignores its attrs argument; subclasses
  # populate from the hash. Item below overrides initialize to honor
  # the hash, mirroring what synthesized model code does.

  class HashItem < Item
    def initialize(attrs = {})
      super()
      # Use the property setter instead of `@title = ...` directly
      # — Crystal infers a per-class `@title : String` here that
      # conflicts with the parent's `@title : String?` (from Item's
      # `attr_accessor :title`). The setter routes through the
      # parent's already-typed property.
      self.title = attrs[:title]
    end
  end

  def test_create_returns_persisted_instance
    h = HashItem.create(title: "Made")
    assert_kind_of HashItem, h
    assert_predicate h, :persisted?
    assert_equal "Made", h.title
  end

  # Named subclass instead of `Class.new(HashItem) do … end` —
  # anonymous-class-with-block isn't representable in roundhouse's
  # IR (no first-class "method def at expression position" node);
  # a lexical subclass keeps the test's spirit AND transpiles
  # naturally to a TS class.
  class FailingHashItem < HashItem
    def self.table_name = "items"
    def validate
      # Explicit `.push(...)` — the bare `@errors << x` shovel form
      # falls through to bit-shift in TS emit when the receiver type
      # is unknown. `errors` (the attr_reader) returns the underlying
      # Array under both CRuby and TS (the TS emit produces a field
      # of the same name; bare-name access is field access).
      errors.push("boom")
    end
  end

  def test_create_bang_raises_record_invalid_when_save_fails
    err = assert_raises(ActiveRecord::RecordInvalid) { FailingHashItem.create!(title: "x") }
    assert_match(/Validation failed/, err.message)
  end

  # ── timestamps (non-trivial: the only Base behavior with state
  #   beyond pure-adapter passthrough) ──────────────────────────

  class Timestamped < ActiveRecord::Base
    attr_accessor :title, :created_at, :updated_at

    def self.table_name = "stamped"
    def self.schema_columns = [:id, :title, :created_at, :updated_at]
    def self.instantiate(row)
      t = new
      # See BaseTest::Item.instantiate above for the cross-target
      # row-cell narrowing rationale.
      t.id = row["id"].to_s.to_i
      title = row["title"]
      created_at = row["created_at"]
      updated_at = row["updated_at"]
      t.title = title if title.is_a?(String)
      t.created_at = created_at if created_at.is_a?(String)
      t.updated_at = updated_at if updated_at.is_a?(String)
      t.mark_persisted!()
      t
    end

    def attributes
      { title: @title, created_at: @created_at, updated_at: @updated_at }.to_h
    end

    def assign_from_row(row)
      title = row["title"]
      created_at = row["created_at"]
      updated_at = row["updated_at"]
      @title = title if title.is_a?(String)
      @created_at = created_at if created_at.is_a?(String)
      @updated_at = updated_at if updated_at.is_a?(String)
    end

    # See BaseTest::Item for the rationale — legacy 12-method-shim
    # opt-in so Base's primitives can stay empty for spinel polymorphic
    # dispatch. Crystal's strict typing also requires concrete returns
    # here: Base's empty `_adapter_insert` returns Nil, but the save
    # path expects Int64; the explicit override threads through to
    # adapter.insert which returns the rowid.
    def _adapter_insert
      ActiveRecord.adapter.insert(Timestamped.table_name, attributes())
    end

    def _adapter_update
      ActiveRecord.adapter.update(Timestamped.table_name, @id, attributes())
    end

    def _adapter_delete
      ActiveRecord.adapter.delete(Timestamped.table_name, @id)
    end

    # Base#fill_timestamps writes via `self[:col] = ...` — provide
    # the index assignment. Hardcoded case dispatch (instead of
    # Ruby's `send("#{key}=", value)` reflection) so the same source
    # transpiles to strict-typed targets — Crystal/Spinel reject
    # dynamic `send` since they can't statically resolve the method.
    def []=(key, value)
      case key
      when :title then self.title = value
      when :created_at then self.created_at = value
      when :updated_at then self.updated_at = value
      end
    end

    def [](key)
      case key
      when :title then @title
      when :created_at then @created_at
      when :updated_at then @updated_at
      end
    end
  end

  def test_save_sets_created_at_and_updated_at_on_insert
    Db.exec("CREATE TABLE stamped (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT, created_at TEXT, updated_at TEXT)")
    t = Timestamped.new
    t.title = "T"
    t.save()
    refute_nil t.created_at
    refute_nil t.updated_at
  end

  def test_save_only_updates_updated_at_on_update
    Db.exec("CREATE TABLE stamped (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT, created_at TEXT, updated_at TEXT)")
    t = Timestamped.new
    t.title = "T"
    t.save()
    original_created = t.created_at
    sleep 0.01 # so the timestamp string actually advances
    t.title = "T2"
    t.save()
    assert_equal original_created, t.created_at
  end
end
