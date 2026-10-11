# frozen_string_literal: true

# One unchanged, independently asserted contract runs in separate processes
# against the original generator, exported core and Roundhouse Ruby output.
require "json"
load ARGV.fetch(0)

def equal(actual, expected)
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless actual == expected
  $checks += 1
  actual
end

$checks = 0
eval_product = EvalProduct.new
left = LeftProduct.new
other_left = LeftProduct.new
right = RightProduct.new
observations = [
  equal(eval_product.compute(11), 53),
  equal(eval_product.compute(-4), -22),
  equal(eval_product.fallback(""), ""),
  equal(eval_product.fallback(0), 0),
  equal(eval_product.fallback(true), true),
  equal(eval_product.fallback(false), "missing"),
  equal(eval_product.fallback(nil), "missing"),
  equal(left.current, 19),
  equal(right.current, 101),
  equal(left.label(41), "λnorth:41"),
  equal(right.label(23), "λsouth:23"),
  equal(left.advance(3), 22),
  equal(other_left.current, 22),
  equal(right.advance(-6), 95),
  equal(left.current, 22),
  equal(other_left.rename("changed"), "changed"),
  equal(left.label(42), "λchanged:42"),
  equal(right.label(24), "λsouth:24"),
  equal(left.flag, false),
  equal(right.flag, nil),
  equal(left.selected("off"), "off"),
  equal(right.selected("nil"), "nil")
]
puts JSON.generate(checks: $checks, observations: observations)
