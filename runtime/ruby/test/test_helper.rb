# Framework-level test bootstrap. Loads the framework Ruby (under
# `runtime/ruby/`). Runs under stock CRuby — no spinel, no transpile,
# no app fixture. Tests check framework-source correctness; transpile-
# correctness is a separate concern handled by per-target tests.
#
# Usage:
#   ruby -Iruntime/ruby runtime/ruby/test/<area>/<thing>_test.rb
# Or via the Rakefile under runtime/ruby/.
#
# Each test file requires this helper, then defines test classes
# that subclass Minitest::Test.
#
# Historical note: prior to <session date> this helper also defined a
# `FrameworkTestAdapter` module — a polymorphic Hash-backed in-memory
# adapter exercised by `runtime/ruby/test/active_record/base_test.rb`.
# That mock has been removed because its `Hash[String, untyped]` shape
# didn't survive spinel monomorphization, and the per-target mirror
# files (`runtime/{crystal,rust,go/v2}/framework_test_adapter.*`) plus
# the TS singleton in `runtime/typescript/juntos.ts` were proliferating
# adapter scaffolding for a single test. A follow-on session will
# re-enable base_test wired against each target's real sqlite adapter
# (CRuby: sqlite3 gem; spinel: libsqlite3 FFI; Crystal: DB::SQLite3;
# TS: better-sqlite3 / libsql; Rust: rusqlite; Go: modernc.org/sqlite).

require "minitest/autorun"

# Base64 / JSON are CRuby stdlib here (the framework tests run under
# stock CRuby with no transpile step). Required up-front so
# action_view/view_helpers's turbo_stream_from has them available
# without inline requires (which spinel-target would warn on).
require "base64"
require "json"

FRAMEWORK_RUBY = File.expand_path("..", __dir__)
$LOAD_PATH.unshift(FRAMEWORK_RUBY)
# `runtime/` itself so `spinel/db_cruby` and `spinel/sqlite_adapter`
# resolve below. Required by base_test.rb's `:memory:` sqlite setup;
# harmless for tests that don't touch persistence.
$LOAD_PATH.unshift(File.expand_path("..", FRAMEWORK_RUBY))

require "active_record"
require "action_view/slots"
require "action_view/view_helpers"
require "action_dispatch/router"
require "action_controller/base"
require "inflector"

# Real Db primitive (gem-backed under CRuby) + SqliteAdapter shim that
# satisfies the AR adapter contract by routing through Db. base_test
# exercises Base CRUD against an in-memory SQLite via these — same
# code path the production sqlite-backed app uses. Other framework
# tests don't touch persistence; load failures are tolerated so this
# helper still works in environments without the sqlite3 gem (the
# `unit` CI job) or without the spinel/ subtree (per-target scratch
# layouts in framework_tests_ruby.rs). base_test.rb checks for `Db`
# being defined and skips itself if these requires didn't take.
begin
  require "spinel/db_cruby"
  require "spinel/sqlite_adapter"
rescue LoadError
  # sqlite3 gem absent OR spinel/ subtree not on load path. base_test
  # is the only consumer; it self-skips when Db is undefined.
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
