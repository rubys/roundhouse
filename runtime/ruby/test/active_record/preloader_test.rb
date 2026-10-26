# frozen_string_literal: true
require_relative "../test_helper"

class PreloadTarget
  def initialize
    @groups = []
  end

  attr_reader :groups

  def _preload_as_group(group, specs)
    @groups << [group, specs]
    nil
  end
end

# The two-keyword call is what the lowerer rewrites. This is the
# runtime the call falls through to, including a scope it refuses.
class PreloaderTest < Minitest::Test
  def test_call_groups_records_and_forwards_the_association
    first = PreloadTarget.new
    second = PreloadTarget.new
    ActiveRecord::Associations::Preloader.new(records: [first, second], associations: :parts).call
    assert_equal [[first, second], [:parts]], first.groups[0]
    assert_empty second.groups
  end

  def test_a_scope_is_refused
    error = assert_raises(ArgumentError) do
      ActiveRecord::Associations::Preloader.new(records: [], associations: :parts, scope: -> { nil })
    end
    assert_match(/scope/, error.message)
  end
end
