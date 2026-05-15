class Article
  attr_accessor :id, :title
end

module Helper
  def self.article_json(article)
    io = String.new
    io << "{" << article.id.to_s << "}"
    io
  end

  def self.index_json(articles)
    io = String.new
    io << "[" << articles.map { |article| Helper.article_json(article) }.join(",") << "]"
    io
  end
end

a = Article.new
a.id = 1
puts Helper.index_json([a])
