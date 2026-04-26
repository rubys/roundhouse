require "action_view"
require "inflector"

module Views
  module Articles
    module_function

    # Lowered shape of fixtures/real-blog/app/views/articles/_article.html.erb.
    # `link_to article.title, article` in the original is the polymorphic
    # short form; lowered here to article_path(article.id) explicitly.
    def article(article)
      io = String.new
      io << %(<div id=")
      io << ViewHelpers.dom_id(article)
      io << %(" class="flex flex-col sm:flex-row justify-between items-center pb-5 sm:pb-0">\n)
      io << %(  <div class="p-4 border rounded mb-4 flex-grow">\n)
      io << %(    <h2 class="text-xl font-bold">\n      )
      io << ViewHelpers.link_to(
        article.title,
        RouteHelpers.article_path(article.id),
        class: "text-blue-600 hover:underline",
      )
      io << "\n      "
      io << %(<span id=")
      io << ViewHelpers.dom_id(article, :comments_count)
      io << %(" class="text-gray-500 text-sm font-normal ml-2">\n)
      io << "        ("
      io << Inflector.pluralize(article.comments.length, "comment")
      io << ")\n      </span>\n    </h2>\n"
      io << %(    <p class="text-gray-700 mt-2">)
      io << ViewHelpers.html_escape(ViewHelpers.truncate(article.body, length: 100))
      io << "</p>\n  </div>\n"
      io << %(  <div class="w-full sm:w-auto flex flex-col sm:flex-row space-x-2 space-y-2">\n    )
      io << ViewHelpers.link_to(
        "Show",
        RouteHelpers.article_path(article.id),
        class: "w-full sm:w-auto text-center rounded-md px-3.5 py-2.5 bg-gray-100 hover:bg-gray-50 inline-block font-medium",
      )
      io << "\n    "
      io << ViewHelpers.link_to(
        "Edit",
        RouteHelpers.edit_article_path(article.id),
        class: "w-full sm:w-auto text-center rounded-md px-3.5 py-2.5 bg-gray-100 hover:bg-gray-50 inline-block font-medium",
      )
      io << "\n    "
      io << ViewHelpers.button_to(
        "Destroy",
        RouteHelpers.article_path(article.id),
        method: :delete,
        class: "w-full sm:w-auto rounded-md px-3.5 py-2.5 text-white bg-red-600 hover:bg-red-500 font-medium cursor-pointer",
        "data-turbo-confirm" => "Are you sure?",
      )
      io << "\n  </div>\n</div>\n"
      io
    end
  end
end
