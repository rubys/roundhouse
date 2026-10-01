expected = [200, "application/json", '{"id":7,"active":false}']
actual = SurveyProbe.call
raise "stub response: #{actual.inspect}" unless actual == expected
puts "PASS status/header/body and stub consumption (no network adapter)"
