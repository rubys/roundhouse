# Expectations are activemodel 8.0's `ActiveModel::Type::Boolean.new.cast`, not hand-derived.
load ARGV.fetch(0)

CASES = [
  [nil, nil],
  ["", nil],
  [" ", true],
  [true, true],
  [false, false],
  [0, false],
  [1, true],
  [0.0, true],
  ["0", false],
  ["1", true],
  ["f", false],
  ["F", false],
  ["false", false],
  ["FALSE", false],
  ["False", true],
  ["off", false],
  ["OFF", false],
  ["Off", true],
  ["no", true],
  ["t", true],
  ["true", true],
  ["on", true],
  [:off, false],
  [:"0", false],
  [:false, false],
  ["x", true],
  [2, true],
].freeze

failures = CASES.reject { |value, want| ActiveSupport.cast_boolean(value) == want }
failures = failures.map { |value, want| "cast_boolean(#{value.inspect}): want #{want.inspect}, got #{ActiveSupport.cast_boolean(value).inspect}" }
failures << "stringify_keys" unless ActiveSupport.stringify_keys({ a: 1, b: 2 }) == { "a" => 1, "b" => 2 }
puts "cases #{CASES.size}"
puts failures
puts "ALL OK" if failures.empty?
exit(failures.empty? ? 0 : 1)
