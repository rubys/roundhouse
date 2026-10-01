author = Struct.new(:id, :name).new(9, "Ada")
article = Struct.new(:id, :title, :active, :author).new(7, "Synthetic", false, author)
actual = SurveyProbe.call(article)
expected = {"id" => 7, "title" => "Synthetic", "active" => false, "author" => {"id" => 9, "name" => "Ada"}}
raise "Panko inherited/nested/false: #{actual.inspect}" unless actual == expected
puts "PASS Panko inherited/nested/false serialization"
