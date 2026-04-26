require "action_view"

module Views
  module Comments
    module_function

    # Lowered shape of fixtures/real-blog/app/views/comments/_comment.html.erb.
    # The `[comment.article, comment]` polymorphic-array form in the
    # original ERB resolves at lowering time to article_comment_path.
    def comment(comment)
      io = String.new
      io << %(<div id=")
      io << ViewHelpers.dom_id(comment)
      io << %(" class="p-4 bg-gray-50 rounded">\n)
      io << %(  <p class="font-semibold">)
      io << ViewHelpers.html_escape(comment.commenter)
      io << "</p>\n"
      io << %(  <p class="text-gray-700">)
      io << ViewHelpers.html_escape(comment.body)
      io << "</p>\n  "
      io << ViewHelpers.button_to(
        "Delete",
        RouteHelpers.article_comment_path(comment.article_id, comment.id),
        method: :delete,
        class: "text-red-600 text-sm mt-2",
        "data-turbo-confirm" => "Are you sure?",
      )
      io << "\n</div>\n"
      io
    end
  end
end
