# Minitest-shaped test base class for emitted Crystal specs. Mirrors
# `runtime/typescript/minitest.ts` and the `Minitest::Test` analog in
# `runtime/ruby/test/test_helper.rb` — same assertion surface
# (`assert_equal`, `assert_nil`, `refute_includes`, `assert_predicate`,
# `assert_kind_of`, …) so transpiled test bodies don't have to know
# they're running under Crystal's `spec` library.
#
# Discovery: each emitted test class invokes `RoundhouseTest.discover`
# at the bottom of its file. The macro walks the class's instance
# methods at compile time, generating one `it "<name>"` Spec block
# per `test_*` method. Each `it` instantiates a fresh test object and
# calls the matching method, mirroring Minitest's per-test isolation.

require "spec"

abstract class RoundhouseTest
  # ── core assertions ──────────────────────────────────────────────

  def assert(cond : Bool, msg : String? = nil) : Nil
    fail(msg || "expected truthy, got false") unless cond
  end

  def assert_not(cond : Bool, msg : String? = nil) : Nil
    fail(msg || "expected false, got truthy") if cond
  end

  def refute(cond : Bool, msg : String? = nil) : Nil
    assert_not(cond, msg)
  end

  def assert_equal(expected, actual, msg : String? = nil) : Nil
    fail(msg || "expected #{expected.inspect}, got #{actual.inspect}") unless expected == actual
  end

  def assert_not_equal(expected, actual, msg : String? = nil) : Nil
    fail(msg || "expected !=, got #{actual.inspect}") if expected == actual
  end

  def refute_equal(expected, actual, msg : String? = nil) : Nil
    assert_not_equal(expected, actual, msg)
  end

  # Ruby `nil` maps to either Crystal `nil` or unset references; accept
  # both as "absent" (analog of the TS shim's null/undefined acceptance).
  def assert_nil(value, msg : String? = nil) : Nil
    fail(msg || "expected nil, got #{value.inspect}") unless value.nil?
  end

  def assert_not_nil(value, msg : String? = nil) : Nil
    fail(msg || "expected non-nil, got nil") if value.nil?
  end

  def refute_nil(value, msg : String? = nil) : Nil
    assert_not_nil(value, msg)
  end

  # Collection emptiness probe — mirrors the TS shim's `collectionSize`:
  # arrays/strings/hashes have native `#empty?`; framework classes
  # (HashWithIndifferentAccess analog) expose `#length` returning Int32.
  # Any responder to `#empty?` matches first.
  def assert_empty(collection, msg : String? = nil) : Nil
    if collection.responds_to?(:empty?)
      fail(msg || "expected empty, got #{collection.inspect}") unless collection.empty?
    elsif collection.responds_to?(:length)
      fail(msg || "expected empty (length 0), got #{collection.inspect}") unless collection.length == 0
    elsif collection.responds_to?(:size)
      fail(msg || "expected empty (size 0), got #{collection.inspect}") unless collection.size == 0
    else
      fail(msg || "assert_empty: unsupported collection type for #{collection.inspect}")
    end
  end

  def assert_not_empty(collection, msg : String? = nil) : Nil
    if collection.responds_to?(:empty?)
      fail(msg || "expected non-empty, got #{collection.inspect}") if collection.empty?
    elsif collection.responds_to?(:length)
      fail(msg || "expected length > 0, got #{collection.inspect}") if collection.length == 0
    else
      fail(msg || "assert_not_empty: unsupported collection type for #{collection.inspect}")
    end
  end

  def refute_empty(collection, msg : String? = nil) : Nil
    assert_not_empty(collection, msg)
  end

  def assert_includes(collection, item, msg : String? = nil) : Nil
    fail(msg || "expected #{collection.inspect} to include #{item.inspect}") unless collection.includes?(item)
  end

  def refute_includes(collection, item, msg : String? = nil) : Nil
    fail(msg || "expected #{collection.inspect} not to include #{item.inspect}") if collection.includes?(item)
  end

  # Accepts `String?` so callers can pass nilable values directly
  # (e.g. `err.message` from a Crystal Exception returns `String?`);
  # nil fails the assertion the same as a non-matching string.
  def assert_match(pattern, value : String?, msg : String? = nil) : Nil
    if value.nil?
      fail(msg || "expected non-nil string to match #{pattern.inspect}")
    end
    re = pattern.is_a?(Regex) ? pattern.as(Regex) : Regex.new(pattern.to_s)
    fail(msg || "expected #{value.inspect} to match #{re.inspect}") unless re.matches?(value)
  end

  # `is_a?` requires a type literal in Crystal — take the class as a
  # macro arg so callers write `assert_kind_of Article, x` and we
  # expand to `x.is_a?(Article)` at compile time.
  macro assert_kind_of(klass, obj, msg = nil)
    %obj = {{obj}}
    fail({{msg}} || "expected #{%obj.inspect} to be a kind of {{klass}}") unless %obj.is_a?({{klass}})
  end

  macro assert_instance_of(klass, obj, msg = nil)
    assert_kind_of({{klass}}, {{obj}}, {{msg}})
  end

  # Ruby's `assert_operator a, :op, b` — eval `a.op(b)` and assert truthy.
  # Symbol-shaped op names (':<', ':>') and the bare form both accepted.
  def assert_operator(left, op, right, msg : String? = nil) : Nil
    op_str = op.to_s.lstrip(':')
    result = case op_str
             when "<"  then left < right
             when ">"  then left > right
             when "<=" then left <= right
             when ">=" then left >= right
             when "==" then left == right
             when "!=" then left != right
             else
               fail(msg || "assert_operator: unsupported op #{op}")
             end
    fail(msg || "expected #{left.inspect} #{op_str} #{right.inspect}") unless result
  end

  # `assert_predicate obj, :foo?` — try `obj.foo?` (Crystal's predicate
  # method form). The TS emit's `is_<name>` rename doesn't apply here
  # since Crystal accepts `?` in method names natively; emit can keep
  # the Ruby form verbatim.
  macro assert_predicate(obj, sym, msg = nil)
    {% name = sym.is_a?(SymbolLiteral) ? sym.id : sym %}
    %obj = {{obj}}
    fail({{msg}} || "expected #{ {{name.stringify}} } to be truthy") unless %obj.{{name}}
  end

  macro refute_predicate(obj, sym, msg = nil)
    {% name = sym.is_a?(SymbolLiteral) ? sym.id : sym %}
    %obj = {{obj}}
    fail({{msg}} || "expected #{ {{name.stringify}} } to be falsy") if %obj.{{name}}
  end

  # `assert_raises(SomeError) { … }` — Crystal's `is_a?` requires a
  # type literal, so this is a macro that captures the class arg at
  # compile time. Returns the raised exception so callers can match
  # on `.message`.
  macro assert_raises(klass, &block)
    begin
      ({{block.body}})
      fail("expected block to raise {{klass}}")
    rescue %ex : {{klass}}
      %ex
    end
  end

  def flunk(msg : String? = nil) : Nil
    fail(msg || "flunked")
  end

  def skip(msg : String? = nil) : Nil
    raise Spec::SpecSkip.new(msg || "skipped", file: __FILE__, line: __LINE__)
  end

  # Bridge the assertion failure into Spec's expectation channel —
  # Spec catches `Spec::AssertionFailed` and reports it as a failed `it`.
  private def fail(msg : String) : Nil
    raise Spec::AssertionFailed.new(msg, file: __FILE__, line: __LINE__)
  end

  # ── discovery macro ──────────────────────────────────────────────
  #
  # Generate `describe <Klass> do … it "test_X" do <Klass>.new.test_X; end … end`
  # at the bottom of the test file. Walks the class's own instance
  # methods at compile time; each `test_*` method becomes one spec.
  # Crystal's `spec` autorun fires when `require "spec"` is loaded and
  # the program reaches main, so the test_helper itself doesn't need
  # an explicit runner.
  macro inherited
    macro finished
      describe \{{ @type }} do
        \{% for m in @type.methods %}
          \{% if m.name.starts_with?("test_") %}
            it \{{ m.name.stringify }} do
              \{{ @type }}.new.\{{ m.name.id }}
            end
          \{% end %}
        \{% end %}
      end
    end
  end
end
