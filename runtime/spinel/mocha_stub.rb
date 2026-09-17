# A mocha stub or expectation on an APP method — the half of mocha
# `lower::mocha`'s table cannot hold a row for, because the method is
# the app's own (`Webhook#post`, `User#reset_remote_connections`), not a
# runtime constant's.
#
# The set of such stubs is closed at transpile time, so the lowering
# gives the METHOD the slot: a `MochaStub` parked on the instance
# (`user.__mocha_reset_remote_connections = MochaStub.expect(...)`, for
# `obj.expects(:m)`) or filed here under the method's name (for
# `Klass.any_instance.stubs(:m)`), and a guard prepended to the lowered
# method that asks for the stub first and, when one is in force, records
# the call, raises what the stub raises, and returns nil in place of the
# body — which is what mocha's replacement does. Same rewrite on every
# lane; the ruby family stops exercising the gem for these shapes and
# runs the slot every other target runs, the parity `lower::mocha` is
# for.
#
# Test-only, ruby-family: required by the test helper, which clears the
# registry in setup and verifies every filed expectation in teardown —
# the same moment, and the same failure, as `mocha_verify`.
class MochaStub
  def initialize(name, expected)
    @name = name
    @expected = expected
    @seen = 0
    @raises = ""
    PENDING.push(self) if expected >= 0
  end

  # `stubs(:m)` — no count to meet.
  def self.stub(name)
    MochaStub.new(name, -1)
  end

  # `expects(:m)` (once), `.never` (0), `.times(n)`.
  def self.expect(name, expected)
    MochaStub.new(name, expected)
  end

  # `.raises(E)` — by class NAME; the lowered guard re-raises the
  # constant itself, one arm per class the chains named for that
  # method, so no name-to-class lookup happens here.
  def raising(class_name)
    @raises = class_name
    self
  end

  def raises
    @raises
  end

  # The guard's first call: one more invocation seen.
  def record!
    @seen = @seen + 1
    nil
  end

  def verify!
    return nil if @expected < 0 || @seen == @expected
    raise "mocha: expected #{@name} #{@expected} time(s), got #{@seen}"
  end

  # `Klass.any_instance.stubs(:m)` — filed by "Klass#m"; the guard in
  # every instance's `m` reads it. Constants as settable holders, the
  # façade slots' idiom (`WebPush::STUB_ON`): a class-level ivar seeded
  # `[]` in one method and pushed in another read as an Integer array
  # on spinel, while a constant is typed by every use it has.
  ANY_INSTANCE = {}
  PENDING = []

  def self.file_any_instance(name, stub)
    ANY_INSTANCE[name] = stub
    stub
  end

  def self.any_instance_for(name)
    ANY_INSTANCE.fetch(name, nil)
  end

  def self.pending_expectations
    PENDING
  end

  def self.clear!
    PENDING.clear
    ANY_INSTANCE.clear
    nil
  end

  def self.verify_all!
    i = 0
    while i < PENDING.length
      PENDING[i].verify!
      i = i + 1
    end
    PENDING.clear
    nil
  end
end
