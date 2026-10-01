require_relative "base_serializer"
require_relative "author_serializer"

class ArticleSerializer < BaseSerializer
  attributes :title, :active
  has_one :author, serializer: AuthorSerializer
end
