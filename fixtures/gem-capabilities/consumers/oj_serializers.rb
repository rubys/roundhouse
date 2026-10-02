# Parameterized APIs are diagnosed in a helper, not a parameterized Rails action.
class SurveysController < ActionController::Base
  def index
    render plain: "analysis-only"
  end

  def serialize(article)
    ArticleSerializer.one(article)
  end
end
