class SurveysController < ActionController::Base
  def index
    JSON.parse(JSON.generate({"active" => false, "missing" => nil, "items" => [7, "x"]}))
  end
end
