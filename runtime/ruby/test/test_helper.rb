# Framework-level test bootstrap. Loads the framework Ruby (under
# `runtime/ruby/`) and provides a tiny in-memory adapter so tests
# can exercise behavior that touches `ActiveRecord.adapter` without
# pulling in a target's storage stack (sqlite gem, better-sqlite3,
# …). Runs under stock CRuby — no spinel, no transpile, no app
# fixture. Tests check framework-source correctness; transpile-
# correctness is a separate concern handled by per-target tests.
#
# Usage:
#   ruby -Iruntime/ruby runtime/ruby/test/<area>/<thing>_test.rb
# Or via the Rakefile under runtime/ruby/.
#
# Each test file requires this helper, then defines test classes
# that subclass Minitest::Test. Per-test isolation comes from the
# adapter's reset between tests; tests that exercise CRUD reset_db
# in their setup.

require "minitest/autorun"

FRAMEWORK_RUBY = File.expand_path("..", __dir__)
$LOAD_PATH.unshift(FRAMEWORK_RUBY)

require "active_record"
require "action_view/view_helpers"
require "action_dispatch/router"
require "action_controller/parameters"

# Tiny in-memory adapter satisfying the 12-method
# `ActiveRecord::Base` contract. Mirrors the semantics of
# `runtime/spinel/in_memory_adapter.rb`; reproduced here as a
# minimal reference implementation so the framework tests don't
# depend on a target-specific runtime tree.
#
# Storage shape: `tables[name] = { id_int => row_hash }`. Rows are
# Symbol-keyed hashes (matches what framework Ruby reads from
# `instantiate(row)`).
module FrameworkTestAdapter
  module_function

  @tables = {}
  @next_ids = {}
  @schemas = {}

  def reset_all!
    @tables = {}
    @next_ids = {}
    @schemas = {}
  end

  def create_table(name, columns:, foreign_keys: [])
    @tables[name] = {}
    @next_ids[name] = 0
    @schemas[name] = { columns: columns, foreign_keys: foreign_keys }
  end

  def drop_table(name)
    @tables.delete(name)
    @next_ids.delete(name)
    @schemas.delete(name)
  end

  def schema(table)
    @schemas[table]
  end

  def truncate(name)
    @tables[name] = {}
    @next_ids[name] = 0
  end

  def find(table, id)
    @tables.fetch(table, {})[id]
  end

  def all(table)
    (@tables[table] || {}).values
  end

  def where(table, conditions)
    all(table).select do |row|
      conditions.all? { |k, v| row[k] == v }
    end
  end

  def count(table)
    (@tables[table] || {}).size
  end

  def exists?(table, id)
    !find(table, id).nil?
  end

  def insert(table, attrs)
    raise "table #{table} not created" unless @tables.key?(table)
    id = (attrs[:id] && attrs[:id] != 0) ? attrs[:id] : (@next_ids[table] += 1)
    @next_ids[table] = [@next_ids[table], id].max
    row = attrs.merge(id: id)
    @tables[table][id] = row
    id
  end

  def update(table, id, attrs)
    return false unless @tables[table]&.key?(id)
    row = @tables[table][id].merge(attrs).merge(id: id)
    @tables[table][id] = row
    true
  end

  def delete(table, id)
    return false unless @tables[table]&.key?(id)
    @tables[table].delete(id)
    true
  end
end

# Reopen Minitest::Test with the AS-flavor assertions framework
# tests need. Keeps the tests' assertion vocabulary consistent
# with the spinel-blog suite + Rails conventions.
class Minitest::Test
  def assert_not(value, msg = nil)
    refute(value, msg)
  end

  def assert_not_nil(value, msg = nil)
    refute_nil(value, msg)
  end
end
