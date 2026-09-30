require_relative "article_resource"

class SurveyProbe
  def self.call
    ArticleResource.new({id: 7, title: "Synthetic", author: {id: 9, name: "Ada"}}).to_h
  end
end
