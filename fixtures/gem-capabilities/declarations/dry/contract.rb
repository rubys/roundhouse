actual = SurveyProbe.call
expected = [
  false, {age: 17, active: false, note: nil}, {age: ["must be adult"]},
  true, {age: 18, active: false, note: nil}, {},
  true, {age: 19, active: true, note: "note"}, {}
]
raise "inherited schema/coercion/threshold: #{actual.inspect}" unless actual == expected
puts "PASS dry declarations inheritance coercion 17/18/19 false/null"
