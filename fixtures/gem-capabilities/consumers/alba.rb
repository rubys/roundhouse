class SurveysController < ActionController::Base
  def index
    ArticleResource.new({id: 7, title: "Synthetic", author: {id: 9, name: "Ada"}}).to_h
  end
end
