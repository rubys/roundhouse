class Article
  attr_accessor :id, :title
end

module Helper
  def self.index_json(articles)
    io = String.new
    io << "[" << articles.map { |article| "{#{article.id}}" }.join(",") << "]"
    io
  end
end

a = Article.new
a.id = 1
puts Helper.index_json([a])
