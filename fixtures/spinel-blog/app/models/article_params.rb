# Per-resource typed Params class. Mirrors what Roundhouse's
# lowerer emits from `params.expect(article: [:title, :body])` in
# real-blog's controller — typed slots replace the
# `Hash[Symbol, untyped]` that's untransparent under whole-program
# inference.
#
# Construction:
#   ArticleParams.from_raw(@params)   — controller path; populates
#                                       every field from request data
#                                       (defaults to "" via fetch).
#   ArticleParams.new                 — programmatic/test path;
#                                       fields default to nil. Followed
#                                       by selective setters; unset
#                                       fields stay nil so update()
#                                       skips them.
class ArticleParams
  attr_accessor :title, :body

  def self.from_raw(params)
    instance = new
    instance.title = params.fetch(:title, "")
    instance.body  = params.fetch(:body, "")
    instance
  end
end
