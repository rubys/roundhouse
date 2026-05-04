require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/active_record/errors.rb`.
# Both error classes are small, but the message-shape contract on
# RecordInvalid matters — every target's transpile relies on the
# `"Validation failed: <joined errors>"` shape (test assertions
# match against substring), and a record with no errors should
# still produce a sensible message.
class RecordNotFoundTest < Minitest::Test
  def test_inherits_from_standard_error
    assert_operator ActiveRecord::RecordNotFound, :<, StandardError
  end

  def test_carries_user_message
    err = ActiveRecord::RecordNotFound.new("Couldn't find Article with id=42")
    assert_equal "Couldn't find Article with id=42", err.message
  end

  def test_default_message_when_constructed_bare
    err = ActiveRecord::RecordNotFound.new
    # Ruby's StandardError.new defaults message to the class name.
    assert_equal "ActiveRecord::RecordNotFound", err.message
  end
end

class RecordInvalidTest < Minitest::Test
  # Stand-in for an unsaved record. RecordInvalid only reads
  # `record.errors`, so any object with that surface works.
  Recordlike = Struct.new(:errors)

  def test_inherits_from_standard_error
    assert_operator ActiveRecord::RecordInvalid, :<, StandardError
  end

  def test_message_joins_errors_with_comma_space
    record = Recordlike.new(["title can't be blank", "body is too short"])
    err = ActiveRecord::RecordInvalid.new(record)
    assert_equal "Validation failed: title can't be blank, body is too short",
      err.message
  end

  def test_message_handles_empty_errors_gracefully
    record = Recordlike.new([])
    err = ActiveRecord::RecordInvalid.new(record)
    assert_equal "Validation failed: ", err.message
  end

  def test_record_attr_exposes_the_offending_record
    record = Recordlike.new(["x"])
    err = ActiveRecord::RecordInvalid.new(record)
    assert_same record, err.record
  end
end
