# frozen_string_literal: true
require_relative "../test_helper"

# Exception classes app code raises and rescues. destroy! does not
# raise RecordNotDestroyed; the class is the rescue target.
class FrameworkExceptionsTest < Minitest::Test
  def test_record_not_destroyed_keeps_its_record
    record = Object.new
    error = ActiveRecord::RecordNotDestroyed.new("still referenced", record)
    assert_equal "still referenced", error.message
    assert_same record, error.record
    rescued = nil
    begin
      raise error
    rescue ActiveRecord::RecordNotDestroyed => caught
      rescued = caught
    end
    assert_same error, rescued
  end

  def test_connection_failures_are_connection_not_established
    assert_operator ActiveRecord::ConnectionFailed, :<, ActiveRecord::ConnectionNotEstablished
    assert_operator ActiveRecord::ConnectionTimeoutError, :<, ActiveRecord::ConnectionNotEstablished
    rescued = nil
    begin
      raise ActiveRecord::ConnectionTimeoutError, "timed out"
    rescue ActiveRecord::ConnectionNotEstablished => caught
      rescued = caught
    end
    assert_instance_of ActiveRecord::ConnectionTimeoutError, rescued
    assert_equal "timed out", rescued.message
  end

  def test_encryption_errors_rescue_as_themselves
    begin
      raise ActiveRecord::Encryption::Errors::Decryption, "bad key"
    rescue ActiveRecord::Encryption::Errors::Configuration
      flunk "configuration rescued a decryption"
    rescue ActiveRecord::Encryption::Errors::Decryption => caught
      assert_equal "bad key", caught.message
    end
  end
end
