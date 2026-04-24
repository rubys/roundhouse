require_relative "active_record/errors"
require_relative "active_record/in_memory_adapter"
require_relative "active_record/validations"
require_relative "active_record/callbacks"
require_relative "active_record/associations"
require_relative "active_record/broadcasts"
require_relative "active_record/base"
require_relative "active_record/migration"

module ActiveRecord
  @adapter = nil

  def self.adapter
    @adapter ||= InMemoryAdapter.new
  end

  def self.adapter=(a)
    @adapter = a
  end

  def self.reset_adapter
    @adapter = InMemoryAdapter.new
  end
end
