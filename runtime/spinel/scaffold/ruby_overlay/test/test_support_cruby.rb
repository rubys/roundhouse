# Test support the CRuby tree can give and the spinel tree cannot:
# loaded last by test/test_helper.rb on the ruby target only
# (`project.rs::apply_cruby_test_support`).
#
# Rails' `ActiveSupport::Testing::ConstantStubbing#stub_const`, as Rails
# writes it: the constant is REPLACED for the block and put back after,
# so code that reads it at call time (`Message::Pagination::PAGE_SIZE`,
# `Opengraph::Fetch::TIMEOUT`) sees the stub. A compiled tree resolves
# constants at build time and has no `const_set`; there the helper is
# absent and the call fails as the gap it is.
class TestBase
  def stub_const(mod, constant, new_value)
    old_value = mod.const_get(constant, false)
    begin
      mod.send(:remove_const, constant)
      mod.const_set(constant, new_value)
      yield
    ensure
      mod.send(:remove_const, constant) if mod.const_defined?(constant, false)
      mod.const_set(constant, old_value)
    end
  end
end
