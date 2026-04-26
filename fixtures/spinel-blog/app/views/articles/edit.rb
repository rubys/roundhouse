require "action_view"
require_relative "_form"

module Views
  module Articles
    module_function

    def edit(article)
      io = String.new
      ViewHelpers.content_for_set(:title, "Editing article")
      io << %(<div class="md:w-2/3 w-full">\n)
      io << %(  <h1 class="font-bold text-4xl">Editing article</h1>\n\n  )
      io << Views::Articles.form(article)
      io << "\n  "
      io << ViewHelpers.link_to(
        "Show this article",
        RouteHelpers.article_path(article.id),
        class: "w-full sm:w-auto text-center mt-2 sm:mt-0 sm:ml-2 rounded-md px-3.5 py-2.5 bg-gray-100 hover:bg-gray-50 inline-block font-medium",
      )
      io << "\n  "
      io << ViewHelpers.link_to(
        "Back to articles",
        RouteHelpers.articles_path,
        class: "w-full sm:w-auto text-center mt-2 sm:mt-0 sm:ml-2 rounded-md px-3.5 py-2.5 bg-gray-100 hover:bg-gray-50 inline-block font-medium",
      )
      io << "\n</div>\n"
      io
    end
  end
end
