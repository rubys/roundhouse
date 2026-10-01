actual = SurveyProbe.call
raise "password comparison: #{actual.inspect}" unless actual == [true, false]
puts "PASS correct/wrong password (test cost 4)"
