def expect_count(label, expected, actual)
  raise "#{label}: expected #{expected}, got #{actual}" if actual != expected
end

# Exercise the emitted inline writers, not an independently formatted seed.
# The two nearby timestamps distinguish preserved microseconds from to_s.
t1 = Time.at(1_700_000_000, 123456).getlocal("-04:00")
t2 = Time.at(1_700_000_000, 123457).getlocal("+09:00")
values = [0.0, -1.25, 1.23456789012345, 1.25e-20, 1.25e20, -1.25e20,
          1.7976931348623157e308, -1.7976931348623157e308,
          2.2250738585072014e-308, 4.9406564584124654e-324]
values.each_with_index do |value, index|
  row = Reading.new
  row.ratio = value
  row.optional_ratio = value
  row.recorded_at = t1
  row.optional_at = t2
  row.save!
  expect_count("float #{index}", 1, row.ratio_matches)
  expect_count("optional float #{index}", 1, row.optional_ratio_matches)
  expect_count("scalar float against nullable column #{index}", 1, row.scalar_optional_ratio_matches)
  expect_count("nullable float against required column #{index}", 1, row.required_ratio_matches_value(value))
  expect_count("timestamp #{index}", index + 1, row.time_matches)
  expect_count("optional timestamp #{index}", index + 1, row.optional_time_matches)
  expect_count("mixed predicate #{index}", 1, row.pair_matches)
end

row = Reading.new
row.ratio = 42.5
row.recorded_at = t2
row.save!
expect_count("nearby timestamp", 1, row.time_matches)
expect_count("nil float", 1, row.optional_ratio_matches)
expect_count("nil timestamp", 1, row.optional_time_matches)
expect_count("nil against required float", 0, row.required_ratio_matches_value(nil))
expect_count("nil against required timestamp", 0, row.required_time_matches_value(nil))
expect_count("value against required timestamp", 1, row.required_time_matches_value(t2))

# The same instant expressed in another timezone matches the stored value;
# the adjoining microsecond remains a separate row.
row.recorded_at = t1.getutc
expect_count("same instant UTC", values.length, row.time_matches)
row.recorded_at = t2.getutc
expect_count("microsecond retained", 1, row.time_matches)
expect_count("direct offset Time", values.length,
             row.time_matches_value(Time.at(1_700_000_000, 123456).getlocal("-04:00")))
puts "typed values: 10 scalar-to-nullable floats, Float/Time nil and non-nil against nullable/required columns, UTC/offset/microseconds, mixed predicates passed"

expect_count("String timestamp stays text", values.length, row.string_time_matches)
expect_count("optional String timestamp", values.length,
             row.string_time_matches_value("2023-11-14 22:13:20.123457"))
expect_count("nil String timestamp", 1, row.string_time_matches_value(nil))
puts "typed values: String timestamp filters stay text; optional String binds NULL"
