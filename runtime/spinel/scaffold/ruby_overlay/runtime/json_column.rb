# frozen_string_literal: true
# The storage boundary for schema-less ActiveRecord json/jsonb columns
# and `serialize …, coder: JSON`. SQLite hands the generated model
# serialized text; Rails exposes the decoded Array/Hash/scalar.
# Hydration writes stored text directly to the backing slot (never
# through dump); application assignment always JSON-encodes, including
# String values, so `"true"` round-trips as a string rather than bool.
require "json"

module JsonColumn
  def self.load(serialized)
    return nil if serialized.nil?
    JSON.parse(serialized)
  end

  def self.dump(value)
    return nil if value.nil?
    JSON.generate(value)
  end
end
