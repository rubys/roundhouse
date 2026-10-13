# frozen_string_literal: true

require "json"
load ARGV.fetch(0)
checks = 0
observations = []
equal = ->(actual, expected) do
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless actual == expected
  checks += 1
  observations << actual
end

product = RailsCoreProduct.new
equal.call(product.name, nil)
equal.call(product.name_describe, "name:")
product.name = "East"
equal.call(product.name, "East")
equal.call(product.name_describe, "name:East")
equal.call(product.title_describe, "name:East")
product.name = "λWest"
equal.call(product.name_describe, "name:λWest")
equal.call(product.title_describe, "name:λWest")
equal.call(RailsCoreProduct.new.name_describe, "name:")
equal.call(product.respond_to?(:attribute_describe), false)
equal.call(CallbackProduct.new.respond_to?(:judge), false)

{
  [2, 6] => [[52, 17, false], [1102, 28, true]],
  [5, 8] => [[115, 19, true], [2425, 30, true]],
  [-4, -9] => [[-74, 2, false], [-1544, 13, true]],
  [0, 1] => [[10, 12, false], [220, 23, true]]
}.each do |(seed, weight), snapshots|
  object = CallbackProduct.new
  object.prime(seed, weight)
  snapshots.each do |expected|
    object.twist
    object.review
    [object.ledger, object.probe_weight, object.allowed?].zip(expected).each do |actual, wanted|
      equal.call(actual, wanted)
    end
  end
end
raise "Rails loaded into exported core" if ARGV.include?("--core") && defined?(ActiveModel)
puts JSON.generate(checks: checks, observations: observations)
