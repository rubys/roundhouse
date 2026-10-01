require_relative "article_serializer"

class SurveyProbe
  def self.call(article)
    ArticleSerializer.new(article).serializable_hash
  end
end
