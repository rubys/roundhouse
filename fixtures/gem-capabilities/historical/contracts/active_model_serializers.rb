author = Struct.new(:id, :name).new(9, "Ada")
article = Struct.new(:id, :title, :active, :author) do
  def read_attribute_for_serialization(name)
    public_send(name)
  end
end.new(7, "Synthetic", false, author)
def author.read_attribute_for_serialization(name)
  public_send(name)
end
actual = SurveyProbe.call(article)
expected = {id: 7, title: "Synthetic", active: false, author: {id: 9, name: "Ada"}}
raise "AMS inherited/nested/false: #{actual.inspect}" unless actual == expected
puts "PASS AMS inherited/nested/false serialization"
