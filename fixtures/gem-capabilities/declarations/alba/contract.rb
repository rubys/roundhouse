actual = SurveyProbe.call
expected = [
  {"id" => 41, "title" => "Draft", "active" => false, "note" => nil,
   "writer" => {"id" => 23, "name" => "Mira"}},
  {"id" => 42, "title" => "Published", "active" => true, "note" => "note",
   "writer" => {"id" => 23, "name" => "Mira"}}
]
raise "declarations/inheritance/presence: #{actual.inspect}" unless actual == expected
puts "PASS Alba declarations inheritance false/null presence"
