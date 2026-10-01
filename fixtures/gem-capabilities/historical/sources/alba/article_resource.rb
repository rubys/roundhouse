require_relative "base_resource"
require_relative "author_resource"

class ArticleResource < BaseResource
  attributes :title
  one :author, resource: AuthorResource
end
