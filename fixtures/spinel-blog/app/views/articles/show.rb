require "action_view"
require_relative "../comments/_comment"
require "models/comment"

module Views
  module Articles
    module_function

    # Lowered shape of fixtures/real-blog/app/views/articles/show.html.erb.
    # `notice` is an explicit param (cf. index.rb's note on flash).
    # The bottom comment-form section is inlined here rather than
    # extracted to a partial — matches the original.
    def show(article, notice: nil)
      io = String.new
      ViewHelpers.content_for_set(:title, "Showing article")
      io << %(<div class="md:w-2/3 w-full">\n)
      if !notice.nil? && !notice.empty?
        io << %(  <p class="py-2 px-3 bg-green-50 mb-5 text-green-500 font-medium rounded-md inline-block" id="notice">)
        io << ViewHelpers.html_escape(notice)
        io << "</p>\n"
      end
      io << "\n"
      io << %(  <h1 class="font-bold text-4xl">)
      io << ViewHelpers.html_escape(article.title)
      io << "</h1>\n\n"
      io << %(  <div class="my-4">\n)
      io << %(    <p class="text-gray-700">)
      io << ViewHelpers.html_escape(article.body)
      io << "</p>\n  </div>\n\n  "
      io << ViewHelpers.link_to(
        "Edit this article",
        RouteHelpers.edit_article_path(article.id),
        class: "w-full sm:w-auto text-center rounded-md px-3.5 py-2.5 bg-gray-100 hover:bg-gray-50 inline-block font-medium",
      )
      io << "\n  "
      io << ViewHelpers.link_to(
        "Back to articles",
        RouteHelpers.articles_path,
        class: "w-full sm:w-auto text-center mt-2 sm:mt-0 sm:ml-2 rounded-md px-3.5 py-2.5 bg-gray-100 hover:bg-gray-50 inline-block font-medium",
      )
      io << "\n  "
      io << ViewHelpers.button_to(
        "Destroy this article",
        RouteHelpers.article_path(article.id),
        method: :delete,
        form_class: "sm:inline-block mt-2 sm:mt-0 sm:ml-2",
        class: "w-full rounded-md px-3.5 py-2.5 text-white bg-red-600 hover:bg-red-500 font-medium cursor-pointer",
        "data-turbo-confirm" => "Are you sure?",
      )
      io << "\n</div>\n\n<hr class=\"my-8\">\n\n"
      io << %(<h2 class="text-xl font-bold mb-4">Comments</h2>\n\n)
      io << ViewHelpers.turbo_stream_from("article_#{article.id}_comments")
      io << "\n\n"
      io << %(<div id="comments" class="space-y-4 mb-8">\n)
      article.comments.each { |c| io << Views::Comments.comment(c) }
      io << "</div>\n\n"
      io << %(<h3 class="text-lg font-semibold mb-2">Add a Comment</h3>\n\n)
      new_comment = Comment.new
      io << ViewHelpers.form_with(
        model: new_comment,
        model_name: "comment",
        action: RouteHelpers.article_comments_path(article.id),
        method: :post,
        opts: { class: "space-y-4" },
      ) do |form|
        body = String.new
        body << "  <div>\n    "
        body << form.label(:commenter, class: "block font-medium")
        body << "\n    "
        body << form.text_field(:commenter, class: "block w-full border rounded p-2")
        body << "\n  </div>\n  <div>\n    "
        body << form.label(:body, class: "block font-medium")
        body << "\n    "
        body << form.text_area(:body, rows: 3, class: "block w-full border rounded p-2")
        body << "\n  </div>\n  "
        body << form.submit("Add Comment", class: "bg-blue-600 text-white px-4 py-2 rounded")
        body << "\n"
        body
      end
      io << "\n"
      io
    end
  end
end
