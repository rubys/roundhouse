expected = {"active" => false, "missing" => nil, "items" => [7, "x"]}
actual = SurveyProbe.call
raise "strict JSON roundtrip: #{actual.inspect}" unless actual == expected
raise "false became null" unless actual["active"] == false
puts "PASS strict JSON false/null/array roundtrip"
