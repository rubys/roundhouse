class SurveyProbe
  def self.call
    JSON.parse(JSON.generate({"active" => false, "missing" => nil, "items" => [7, "x"]}))
  end
end
