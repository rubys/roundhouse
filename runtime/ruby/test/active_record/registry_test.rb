require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/active_record/registry.rb`.
# The registry is a tiny module-singleton — register / lookup /
# registered? / clear!. The shape matters because Sessions 3 + 4
# (rust2 / go2 consumption per issue #20) bind their cycle-resolution
# paths to this exact API.
class RegistryTest < Minitest::Test
  def setup
    ActiveRecord::Registry.clear!
  end

  def teardown
    ActiveRecord::Registry.clear!
  end

  def test_lookup_returns_registered_value
    klass = Class.new
    ActiveRecord::Registry.register("Comment", klass)
    assert_same klass, ActiveRecord::Registry.lookup("Comment")
  end

  def test_lookup_returns_nil_when_missing
    assert_nil ActiveRecord::Registry.lookup("Nope")
  end

  def test_registered_predicate
    refute ActiveRecord::Registry.registered?("Article")
    ActiveRecord::Registry.register("Article", Class.new)
    assert ActiveRecord::Registry.registered?("Article")
  end

  def test_register_is_idempotent_and_overwrites
    first = Class.new
    second = Class.new
    ActiveRecord::Registry.register("Comment", first)
    ActiveRecord::Registry.register("Comment", second)
    assert_same second, ActiveRecord::Registry.lookup("Comment")
  end

  def test_clear_removes_every_entry
    ActiveRecord::Registry.register("Article", Class.new)
    ActiveRecord::Registry.register("Comment", Class.new)
    ActiveRecord::Registry.clear!
    assert_nil ActiveRecord::Registry.lookup("Article")
    assert_nil ActiveRecord::Registry.lookup("Comment")
    refute ActiveRecord::Registry.registered?("Article")
  end

  def test_register_accepts_any_untyped_value
    # The registry's value type is `untyped` — Ruby/Spinel/Crystal
    # callers store the class itself; Rust/Go store factory closures
    # or trait objects. The contract is "whatever the target needs."
    ActiveRecord::Registry.register("Sym", :symbol_value)
    ActiveRecord::Registry.register("Num", 42)
    ActiveRecord::Registry.register("Proc", ->(x) { x * 2 })
    assert_equal :symbol_value, ActiveRecord::Registry.lookup("Sym")
    assert_equal 42, ActiveRecord::Registry.lookup("Num")
    assert_equal 10, ActiveRecord::Registry.lookup("Proc").call(5)
  end
end
