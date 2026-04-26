require "action_view"
require_relative "_article"

module Views
  module Articles
    module_function

    # Lowered shape of fixtures/real-blog/app/views/articles/index.html.erb.
    # `notice` (a controller-set flash message) is passed in explicitly;
    # nil means "no flash to display." Real Rails reads it via a
    # `flash` helper; the lowered form makes it an explicit parameter.
    def index(articles, notice: nil)
      io = String.new
      io << ViewHelpers.turbo_stream_from("articles")
      io << "\n\n"
      ViewHelpers.content_for_set(:title, "Articles")
      io << %(<div class="w-full">\n)
      if !notice.nil? && !notice.empty?
        io << %(  <p class="py-2 px-3 bg-green-50 mb-5 text-green-500 font-medium rounded-md inline-block" id="notice">)
        io << ViewHelpers.html_escape(notice)
        io << "</p>\n"
      end
      io << %(\n  <div class="flex justify-between items-center">\n)
      io << %(    <h1 class="font-bold text-4xl">Articles</h1>\n    )
      io << ViewHelpers.link_to(
        "New article",
        RouteHelpers.new_article_path,
        class: "rounded-md px-3.5 py-2.5 bg-blue-600 hover:bg-blue-500 text-white block font-medium",
      )
      io << "\n  </div>\n\n"
      io << %(  <div id="articles" class="min-w-full divide-y divide-gray-200 space-y-5">\n)
      if articles.empty?
        io << %(    <p class="text-center my-10">No articles found.</p>\n)
      else
        articles.each { |a| io << Views::Articles.article(a) }
      end
      io << "  </div>\n</div>\n"
      io
    end
  end
end
