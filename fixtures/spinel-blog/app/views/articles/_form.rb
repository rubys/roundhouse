require "action_view"
require "inflector"

module Views
  module Articles
    module_function

    # Lowered shape of fixtures/real-blog/app/views/articles/_form.html.erb.
    # `form_with(model: article)` in the original resolves to either
    # POST /articles (new record) or PATCH /articles/:id (existing).
    def form(article)
      action = article.persisted? ? RouteHelpers.article_path(article.id) : RouteHelpers.articles_path
      method = article.persisted? ? :patch : :post

      ViewHelpers.form_with(
        model: article,
        model_name: "article",
        action: action,
        method: method,
        opts: { class: "contents" },
      ) do |form|
        body = String.new
        if !article.errors.empty?
          body << %(  <div id="error_explanation" class="bg-red-50 text-red-500 px-3 py-2 font-medium rounded-md mt-3">\n)
          body << "    <h2>"
          body << Inflector.pluralize(article.errors.length, "error")
          body << " prohibited this article from being saved:</h2>\n\n"
          body << %(    <ul class="list-disc ml-6">\n)
          article.errors.each do |err|
            body << "      <li>"
            body << ViewHelpers.html_escape(err)
            body << "</li>\n"
          end
          body << "    </ul>\n  </div>\n"
        end
        body << %(\n  <div class="my-5">\n    )
        body << form.label(:title)
        body << "\n    "
        body << form.text_field(:title, class: "block shadow-sm rounded-md border px-3 py-2 mt-2 w-full")
        body << "\n  </div>\n\n"
        body << %(  <div class="my-5">\n    )
        body << form.label(:body)
        body << "\n    "
        body << form.text_area(:body, rows: 4, class: "block shadow-sm rounded-md border px-3 py-2 mt-2 w-full")
        body << "\n  </div>\n\n"
        body << %(  <div class="inline">\n    )
        body << form.submit(nil, class: "w-full sm:w-auto rounded-md px-3.5 py-2.5 bg-blue-600 hover:bg-blue-500 text-white inline-block font-medium cursor-pointer")
        body << "\n  </div>\n"
        body
      end
    end
  end
end
