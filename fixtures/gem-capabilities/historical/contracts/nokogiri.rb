expected = ["Alpha & Beta", 2]
actual = SurveyProbe.call
raise "XML selection/entity/count: #{actual.inspect}" unless actual == expected
puts "PASS XML predicate/entity/count"
