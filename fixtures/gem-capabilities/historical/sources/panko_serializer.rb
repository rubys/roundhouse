require_relative "article_serializer"

class SurveyProbe
  def self.call(article)
    ArticleSerializer.new.serialize(article)
  end
end
