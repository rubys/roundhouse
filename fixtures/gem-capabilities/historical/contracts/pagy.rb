actual = SurveyProbe.new.last_page
raise "last page: #{actual.inspect}" unless actual == [[55], 5, 5, 3, nil]
actual = SurveyProbe.new.overflow
raise "overflow: #{actual.inspect}" unless actual == []
puts "PASS short last page and overflow empty array"
