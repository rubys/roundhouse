actual = SurveyProbe.call
expected = [true, {age: 21}, false, {age: ["must be adult"]}]
raise "validation/coercion/rule: #{actual.inspect}" unless actual == expected
puts "PASS valid age and rejected age with exact error"
