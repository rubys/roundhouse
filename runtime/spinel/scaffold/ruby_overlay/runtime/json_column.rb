# The storage boundary for schema-less ActiveRecord json/jsonb columns.
# SQLite hands the generated model serialized text; Rails exposes the
# decoded Array/Hash/scalar and accepts either that logical value or raw
# text during hydration.
require "json"

module JsonColumn
  def self.load(serialized)
    return nil if serialized.nil?
    JSON.parse(serialized)
  end

  def self.dump(value)
    return nil if value.nil?
    return value if value.is_a?(String)
    JSON.generate(value)
  end
end
