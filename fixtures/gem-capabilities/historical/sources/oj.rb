class SurveyProbe
  def self.call
    Oj.load(Oj.dump({"active" => false, "missing" => nil, "items" => [7, "x"]}, mode: :strict), mode: :strict)
  end
end
