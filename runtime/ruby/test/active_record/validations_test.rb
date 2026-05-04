require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/active_record/validations.rb`.
# Each `validates_*` helper gets exercised against typed values plus
# the boundary cases (nil, empty string, wrong type) so changes that
# break a predicate surface here, not via downstream
# transpile-and-run on a target.
class ValidationsTest < Minitest::Test
  # Test model — the simplest possible class that mixes in
  # Validations. No DB persistence needed for the presence/length/
  # format/etc. helpers; only validates_belongs_to touches the
  # adapter (existence check).
  class Validatable
    include ActiveRecord::Validations
    attr_accessor :errors

    def initialize
      @errors = []
    end
  end

  def setup
    @subject = Validatable.new
  end

  # ── presence ────────────────────────────────────────────────

  def test_validates_presence_of_passes_on_non_blank_string
    @subject.validates_presence_of(:title, "Hello")
    assert_empty @subject.errors
  end

  def test_validates_presence_of_fails_on_nil
    @subject.validates_presence_of(:title, nil)
    assert_includes @subject.errors, "title can't be blank"
  end

  def test_validates_presence_of_fails_on_empty_string
    @subject.validates_presence_of(:title, "")
    assert_includes @subject.errors, "title can't be blank"
  end

  def test_validates_presence_of_fails_on_empty_array
    @subject.validates_presence_of(:tags, [])
    assert_includes @subject.errors, "tags can't be blank"
  end

  def test_validates_presence_of_passes_on_zero
    @subject.validates_presence_of(:count, 0)
    assert_empty @subject.errors
  end

  # ── absence ─────────────────────────────────────────────────

  def test_validates_absence_of_passes_on_nil
    @subject.validates_absence_of(:deleted_at, nil)
    assert_empty @subject.errors
  end

  def test_validates_absence_of_fails_on_present_string
    @subject.validates_absence_of(:deleted_at, "x")
    assert_includes @subject.errors, "deleted_at must be blank"
  end

  # ── length ──────────────────────────────────────────────────

  def test_validates_length_of_passes_within_range
    @subject.validates_length_of(:body, "abcdefghij", minimum: 5, maximum: 20)
    assert_empty @subject.errors
  end

  def test_validates_length_of_fails_below_minimum
    @subject.validates_length_of(:body, "abc", minimum: 5)
    assert_includes @subject.errors, "body is too short (minimum is 5)"
  end

  def test_validates_length_of_fails_above_maximum
    @subject.validates_length_of(:body, "abcdefghij", maximum: 5)
    assert_includes @subject.errors, "body is too long (maximum is 5)"
  end

  def test_validates_length_of_fails_on_wrong_exact_length
    @subject.validates_length_of(:zip, "1234", is: 5)
    assert_includes @subject.errors, "zip is the wrong length (should be 5)"
  end

  def test_validates_length_of_passes_on_array
    @subject.validates_length_of(:tags, [1, 2, 3], minimum: 1, maximum: 5)
    assert_empty @subject.errors
  end

  def test_validates_length_of_skips_on_nil_value
    @subject.validates_length_of(:body, nil, minimum: 5)
    assert_empty @subject.errors
  end

  # ── numericality ────────────────────────────────────────────

  def test_validates_numericality_of_passes_on_int
    @subject.validates_numericality_of(:age, 25, greater_than: 0)
    assert_empty @subject.errors
  end

  def test_validates_numericality_of_fails_on_string
    @subject.validates_numericality_of(:age, "abc")
    assert_includes @subject.errors, "age is not a number"
  end

  def test_validates_numericality_of_fails_below_greater_than
    @subject.validates_numericality_of(:age, 0, greater_than: 0)
    assert_includes @subject.errors, "age must be greater than 0"
  end

  def test_validates_numericality_of_only_integer_rejects_float
    @subject.validates_numericality_of(:count, 3.14, only_integer: true)
    assert_includes @subject.errors, "count must be an integer"
  end

  # ── inclusion ───────────────────────────────────────────────

  def test_validates_inclusion_of_passes_when_member
    @subject.validates_inclusion_of(:status, "active", within: %w[active inactive])
    assert_empty @subject.errors
  end

  def test_validates_inclusion_of_fails_when_not_member
    @subject.validates_inclusion_of(:status, "deleted", within: %w[active inactive])
    assert_includes @subject.errors, "status is not included in the list"
  end

  # ── format ──────────────────────────────────────────────────

  def test_validates_format_of_passes_on_match
    @subject.validates_format_of(:zip, "12345", with: /\A\d{5}\z/)
    assert_empty @subject.errors
  end

  def test_validates_format_of_fails_on_mismatch
    @subject.validates_format_of(:zip, "abcd", with: /\A\d{5}\z/)
    assert_includes @subject.errors, "zip is invalid"
  end

  # ── belongs_to (touches the adapter) ────────────────────────
  #
  # validates_belongs_to dispatches `target_class.exists?(fk_value)`.
  # `exists?` on Base calls `ActiveRecord.adapter.exists?(...)`.
  # Adapter contract defines this method; if the adapter doesn't
  # expose it under the same name, this test fails — catching
  # cross-target adapter-contract drift at the framework layer.

  class StubModel < ActiveRecord::Base
    def self.table_name = "stubs"
    def self.schema_columns = [:id]
    def self.instantiate(row); s = new; s.id = row[:id]; s.mark_persisted!; s; end
  end

  def setup_adapter_with_stub_row(id)
    ActiveRecord.adapter = FrameworkTestAdapter
    FrameworkTestAdapter.reset_all!
    FrameworkTestAdapter.create_table("stubs", columns: [:id])
    FrameworkTestAdapter.insert("stubs", id: id)
  end

  def test_validates_belongs_to_passes_when_target_exists
    setup_adapter_with_stub_row(7)
    @subject.validates_belongs_to(:owner, 7, StubModel)
    assert_empty @subject.errors
  end

  def test_validates_belongs_to_fails_on_nil_fk
    @subject.validates_belongs_to(:owner, nil, StubModel)
    assert_includes @subject.errors, "owner must exist"
  end

  def test_validates_belongs_to_fails_on_zero_fk
    @subject.validates_belongs_to(:owner, 0, StubModel)
    assert_includes @subject.errors, "owner must exist"
  end

  def test_validates_belongs_to_fails_when_target_missing
    setup_adapter_with_stub_row(7)
    @subject.validates_belongs_to(:owner, 999, StubModel)
    assert_includes @subject.errors, "owner must exist"
  end
end
