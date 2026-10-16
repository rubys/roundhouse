# frozen_string_literal: true

module ActiveRecord
  # Not in base.rb: the strict targets have no `record.class` dispatch to a class method, so only the ruby family and spinel load this.
  module Preloading
    def self.records(records, specs)
      records.each_with_index do |record, i|
        # Not a Hash keyed by class: analysis types `record.class` as a bare Class, which answers no class method.
        next if records.take(i).any? { |earlier| earlier.class == record.class }
        record._preload_as_group(records.select { |other| other.class == record.class }, specs)
      end
      nil
    end
  end

  class Base
    def _preload_as_group(group, specs)
      self.class.preload_associations(group, specs)
    end
  end
end
