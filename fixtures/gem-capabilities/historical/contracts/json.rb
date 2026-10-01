expected = {"active" => false, "missing" => nil, "items" => [7, "x"]}
actual = SurveyProbe.call
raise "JSON roundtrip: #{actual.inspect}" unless actual == expected
raise "false became null" unless actual["active"] == false
puts "PASS JSON false/null/array roundtrip"
