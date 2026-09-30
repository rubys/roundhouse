actual = SurveyProbe.call
raise "ZIP name/payload/EOF: #{actual.inspect}" unless actual == ["nested/synthetic.txt", "ZIP payload\n", nil]
puts "PASS in-memory ZIP name/payload/EOF roundtrip"
