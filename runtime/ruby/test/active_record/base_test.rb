require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/active_record/base.rb`. Each
# CRUD method (find/all/where/count/exists?/save/destroy/reload/
# create/create!/last) gets exercised against `FrameworkTestAdapter`
# (the in-memory adapter shipped with test_helper.rb). A subclass
# defines the per-model overrides Base requires (`table_name`,
# `schema_columns`, `instantiate`, `attributes`, `assign_from_row`)
# so the framework dispatch + lifecycle is what's under test, not
# the synthesized model code.
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
      it.id = row[:id]
      it.title = row[:title]
      it.mark_persisted!
      it
    end

    def attributes
      { title: @title }
    end

    def assign_from_row(row)
      @title = row[:title]
    end
  end

  def setup
    ActiveRecord.adapter = FrameworkTestAdapter
    FrameworkTestAdapter.reset_all!
    FrameworkTestAdapter.create_table("items", columns: [:id, :title])
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
    assert item.save
    assert_predicate item, :persisted?
    refute_predicate item, :new_record?
  end

  def test_save_assigns_id
    item = Item.new
    item.title = "Hi"
    item.save
    refute_equal 0, item.id
  end

  # ── class-level finders ─────────────────────────────────────

  def test_find_returns_instance_for_existing_row
    a = Item.new; a.title = "A"; a.save
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
    a = Item.new; a.title = "A"; a.save
    b = Item.new; b.title = "B"; b.save

    found = Item.find_by(title: "B")
    assert_equal b.id, found.id

    miss = Item.find_by(title: "Nope")
    assert_nil miss
  end

  def test_where_filters_to_matching_rows
    a = Item.new; a.title = "A"; a.save
    b = Item.new; b.title = "B"; b.save
    c = Item.new; c.title = "A"; c.save

    matches = Item.where(title: "A")
    assert_equal 2, matches.length
    assert(matches.all? { |m| m.title == "A" })
  end

  def test_all_returns_every_row
    3.times { |i| it = Item.new; it.title = "T#{i}"; it.save }
    assert_equal 3, Item.all.length
  end

  def test_count_matches_all_size
    2.times { |i| it = Item.new; it.title = "T#{i}"; it.save }
    assert_equal 2, Item.count
  end

  def test_exists_returns_true_for_present_id_false_for_absent
    a = Item.new; a.title = "A"; a.save
    assert Item.exists?(a.id)
    refute Item.exists?(a.id + 9999)
  end

  def test_last_returns_highest_id_or_nil_when_empty
    assert_nil Item.last
    a = Item.new; a.title = "A"; a.save
    b = Item.new; b.title = "B"; b.save
    assert_equal b.id, Item.last.id
  end

  # ── update + destroy ────────────────────────────────────────

  def test_save_updates_existing_record
    a = Item.new; a.title = "Original"; a.save
    fetched = Item.find(a.id)
    fetched.title = "Updated"
    fetched.save
    refetched = Item.find(a.id)
    assert_equal "Updated", refetched.title
  end

  def test_destroy_removes_row_and_marks_destroyed
    a = Item.new; a.title = "Doomed"; a.save
    a.destroy
    assert_predicate a, :destroyed?
    assert_raises(ActiveRecord::RecordNotFound) { Item.find(a.id) }
  end

  def test_destroy_on_unpersisted_returns_self_without_touching_db
    item = Item.new
    item.destroy
    refute_predicate item, :destroyed?
  end

  def test_reload_refreshes_from_db
    a = Item.new; a.title = "First"; a.save
    # Mutate via a separate fetch to simulate concurrent change.
    other = Item.find(a.id)
    other.title = "Second"
    other.save

    a.reload
    assert_equal "Second", a.title
  end

  def test_destroy_all_removes_every_row
    3.times { |i| it = Item.new; it.title = "T#{i}"; it.save }
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
      super
      @title = attrs[:title]
    end
  end

  def test_create_returns_persisted_instance
    h = HashItem.create(title: "Made")
    assert_kind_of HashItem, h
    assert_predicate h, :persisted?
    assert_equal "Made", h.title
  end

  def test_create_bang_raises_record_invalid_when_save_fails
    klass = Class.new(HashItem) do
      def self.table_name = "items"
      def validate
        @errors << "boom"
      end
    end
    err = assert_raises(ActiveRecord::RecordInvalid) { klass.create!(title: "x") }
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
      t.id = row[:id]
      t.title = row[:title]
      t.created_at = row[:created_at]
      t.updated_at = row[:updated_at]
      t.mark_persisted!
      t
    end

    def attributes
      { title: @title, created_at: @created_at, updated_at: @updated_at }
    end

    def assign_from_row(row)
      @title = row[:title]
      @created_at = row[:created_at]
      @updated_at = row[:updated_at]
    end

    # Base#fill_timestamps writes via `self[:col] = ...` — provide
    # the index assignment.
    def []=(key, value)
      send("#{key}=", value)
    end

    def [](key)
      send(key)
    end
  end

  def test_save_sets_created_at_and_updated_at_on_insert
    FrameworkTestAdapter.create_table("stamped", columns: [:id, :title, :created_at, :updated_at])
    t = Timestamped.new
    t.title = "T"
    t.save
    refute_nil t.created_at
    refute_nil t.updated_at
  end

  def test_save_only_updates_updated_at_on_update
    FrameworkTestAdapter.create_table("stamped", columns: [:id, :title, :created_at, :updated_at])
    t = Timestamped.new
    t.title = "T"
    t.save
    original_created = t.created_at
    sleep 0.01 # so the timestamp string actually advances
    t.title = "T2"
    t.save
    assert_equal original_created, t.created_at
  end
end
