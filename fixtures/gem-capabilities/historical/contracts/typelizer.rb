actual = SurveyProbe.call
expected = [["id", :number, false, false], ["nickname", :string, true, true]]
raise "Typelizer explicit types: #{actual.inspect}" unless actual == expected
puts "PASS Typelizer Alba explicit number/optional/nullable metadata (no model/DB inference)"
