# frozen_string_literal: true
# The storage boundary for PostgreSQL array columns (`array: true`).
# SQLite holds the Array as JSON text; the record keeps the decoded
# Array, so an in-place `labels << x` is what the next save writes.
# Elements come back as the column's type: integers or strings.
require "json"

module ArrayColumn
  def self.load_ints(text)
    return nil if text.nil?
    JSON.parse(text).map { |element| element.to_i }
  end

  def self.load_strings(text)
    return nil if text.nil?
    JSON.parse(text).map { |element| element.to_s }
  end

  def self.dump(value)
    return nil if value.nil?
    JSON.generate(value)
  end
end
