class Article < ApplicationRecord
  has_many :comments

  def self.counted
    left_joins(:comments).select("articles.*, COUNT(comments.id) AS comments_total")
      .group("articles.id").order(:id)
  end

  def self.string_keys
    counted.map { |article| "#{article["title"]}=#{article["comments_total"].to_i}" }
  end

  def self.symbol_keys
    counted.map { |article| article[:comments_total].to_i }
  end

  def self.self_reads
    counted.map { |article| article.own_total.to_i }
  end

  def self.unselected
    plain = order(:id).first
    [plain[:comments_total].nil?, plain["comments_total"].nil?, counted.first[:missing].nil?]
  end

  def self.renamed(id)
    article = find(id)
    article["title"] = "Renamed"
    article.title
  end

  def own_total
    self["comments_total"]
  end
end
