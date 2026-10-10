require_relative "../test_helper"
require "security_utils"

class ActiveSupportSecurityUtilsTest < Minitest::Test
  def test_fixed_length_secure_compare_checks_all_bytes
    assert ActiveSupport::SecurityUtils.fixed_length_secure_compare("abc", "abc")
    refute ActiveSupport::SecurityUtils.fixed_length_secure_compare("abc", "abd")
    assert ActiveSupport::SecurityUtils.fixed_length_secure_compare("", "")
  end

  def test_fixed_length_secure_compare_uses_byte_lengths
    assert ActiveSupport::SecurityUtils.fixed_length_secure_compare("é", "\xc3\xa9".b)
    refute ActiveSupport::SecurityUtils.fixed_length_secure_compare("é", "ê")

    error = assert_raises(ArgumentError) do
      ActiveSupport::SecurityUtils.fixed_length_secure_compare("é", "x")
    end
    assert_equal "string length mismatch.", error.message
  end
end
