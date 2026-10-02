expected = {"id" => 7, "title" => "Synthetic", "author" => {"id" => 9, "name" => "Ada"}}
actual = SurveyProbe.call
raise "inherited/nested serialization: #{actual.inspect}" unless actual == expected
puts "PASS inherited/nested serialization"
