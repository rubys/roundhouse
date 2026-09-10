# The mocha bridge — the strict-target half.
#
# `lower::mocha` serves the stub shapes its table knows with typed slots
# (`Resolv.stub_getaddresses`, `WebPush.expect_payload_send`, …). A
# chain it cannot serve is not left spelled as mocha — on this target
# `stubs` is a frontend refusal, and one such line took a whole test
# FILE off the compiled lane — but handed here AS DATA: the constant by
# name, `:stubs`/`:expects` (or the `any_instance_` forms), the method,
# and the chain's links as `[[:with, [args]], [:returns, [v]], …]`, with
# a `with` block riding on the call.
#
# This file is what a target with no method table to swap does with
# that: raise, per test, at the call. The ruby family gets a different
# file under the same name (`project::MOCHA_BRIDGE_REPLAY`) that replays
# the chain through the real gem, so nothing those lanes passed changes.
#
# A test that reaches one of these fails on its own line rather than
# taking its file with it, which is the difference between "17 tests
# blocked" and "the 5 that actually stub".
module MochaBridge
  def self.chain(konst, kind, meth, ops, &blk)
    raise NotImplementedError,
          "mocha: #{konst}.#{kind}(:#{meth}) with #{ops.length} chained link(s) is not a stub shape this target serves"
  end

  # A bare parameter matcher (`has_entry(...)`, `anything`) from
  # `Mocha::ParameterMatchers`, by name.
  def self.matcher(name, arg = nil)
    raise NotImplementedError, "mocha: the #{name} parameter matcher is not served on this target"
  end
end
