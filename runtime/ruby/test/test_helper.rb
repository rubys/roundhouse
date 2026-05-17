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

# Base64 / JSON are CRuby stdlib here (the framework tests run under
# stock CRuby with no transpile step). Required up-front so
# action_view/view_helpers's turbo_stream_from has them available
# without inline requires (which spinel-target would warn on).
require "base64"
require "json"

FRAMEWORK_RUBY = File.expand_path("..", __dir__)
$LOAD_PATH.unshift(FRAMEWORK_RUBY)

require "active_record"
require "action_view/view_helpers"
require "action_dispatch/router"
require "action_controller/base"
require "inflector"

# Tiny in-memory adapter satisfying the 12-method
# `ActiveRecord::Base` contract. Mirrors the semantics of
# `runtime/crystal/framework_test_adapter.cr`; reproduced here as a
# minimal reference implementation so the framework tests don't
# depend on a target-specific runtime tree.
#
# Storage shape: `tables[name] = { id_int => row_hash }`. Rows are
# String-keyed hashes — matches the production sqlite adapters
# across targets (Crystal SqliteAdapter returns `Hash(String, DB::Any)`,
# TS adapters return `{[k: string]: V}`), so test fixtures can
# `row["id"]` against rows from either adapter.
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

  # Conditions come in Symbol-keyed (the Hash<Symbol, _> contract
  # ActiveRecord::Base#where passes through); rows are String-keyed.
  # Compare via stringified keys.
  def where(table, conditions)
    all(table).select do |row|
      conditions.all? { |k, v| row[k.to_s] == v }
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
    row = attrs.transform_keys(&:to_s).merge("id" => id)
    @tables[table][id] = row
    id
  end

  def update(table, id, attrs)
    return false unless @tables[table]&.key?(id)
    row = @tables[table][id].merge(attrs.transform_keys(&:to_s)).merge("id" => id)
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
# with the spinel-blog suite + Rails conventions. Used by the
# bare-source `framework_ruby_tests_pass` gate (CRuby + Minitest
# autorun).
class Minitest::Test
  def assert_not(value, msg = nil)
    refute(value, msg)
  end

  def assert_not_nil(value, msg = nil)
    refute_nil(value, msg)
  end
end

# Roundhouse-owned test parent. Used by `framework_tests_ruby` — the
# gate that ingests these same test files, runs `emit_spinel` over
# them, and then executes the emitted output under CRuby. The emit
# rewrites `class XTest < Minitest::Test` to `< TestBase` so the
# per-test shim's zero-arg `XTest.new` works (Minitest::Test's
# `initialize(name)` requires a method-name argument). Same shape
# as `runtime/spinel/test/test_helper.rb`'s TestBase — uniform
# across both ruby targets, no Minitest dependency in the emit path.
class TestBase
  def initialize
  end

  def setup
  end

  def teardown
  end

  # `assert_operator a, :op, b` is deliberately not lowered by
  # inline_assertions (Class-subclass `<`/`>` checks don't translate
  # to TS, which has no operator-on-class-object equivalent). Each
  # target's test_helper provides the method natively; here under
  # CRuby we just delegate to the operator method.
  def assert_operator(lhs, op, rhs, msg = nil)
    return if lhs.send(op, rhs)
    raise(msg || "assert_operator failed: #{lhs.inspect} #{op} #{rhs.inspect}")
  end

  # Not lowered for the same nilable-value reason as assert_operator —
  # the cross-target-safe form would need per-target regex API
  # handling. Ruby's `=~` handles nil values cleanly (nil =~ /.../
  # returns nil = falsy); each target's test_helper provides its own
  # method.
  def assert_match(pattern, value, msg = nil)
    raise(msg || "assert_match: expected non-nil") if value.nil?
    return if value =~ pattern
    raise(msg || "assert_match failed: expected #{value.inspect} to match #{pattern.inspect}")
  end
end
