# Portable source-property contract, not the survey's original hash-input case.
class AlbaAuthor
  def initialize(id, name)
    @id = id
    @name = name
  end

  def id
    @id || raise("id required")
  end

  def name
    @name || raise("name required")
  end
end

class AlbaArticle
  def initialize(id, title, author)
    @id = id
    @title = title
    @author = author
  end

  def id
    @id || raise("id required")
  end

  def title
    title = @title || raise("title required")
    title + "thetic"
  end

  # The initial portable contract requires a non-null association. Make that
  # source invariant explicit; nullable associations are a separate boundary.
  def author
    @author || raise("author required")
  end
end

class BaseResource
  include Alba::Resource
  attributes :id
end

class AuthorResource < BaseResource
  attributes :name
end

class ArticleResource < BaseResource
  attributes :title
  one :author, resource: AuthorResource
end

class SurveyProbe
  def self.call
    author = AlbaAuthor.new(9, "Ada")
    article = AlbaArticle.new(7, "Syn", author)
    ArticleResource.new(article).to_h
  end
end
