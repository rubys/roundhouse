// GENERATED-CODE reference for the views — transcribed from the lowered IR
// (dump_ir Views::Articles#index and #article). Ruby's `io = String.new;
// io << "..."` lowers to the StringBuilder hint, which in Swift is plain
// string accumulation (`var io = ""` / `io += chunk` / `io` — String
// append is amortized O(1)); `#{...}` interpolations become Swift
// `\(...)`; helper calls keep their lowered call shape (`ViewHelpers.*`,
// `RouteHelpers.*`, `Inflector.*`). The HTML string literals are
// reproduced verbatim — this is the byte-for-byte spec the emitter must
// reproduce.
enum ArticlesView {
    static func index(_ articles: [Article], _ notice: String? = nil, _ alert: String? = nil) -> String {
        var io = ""
        io += "\(ViewHelpers.turboStreamFrom("articles"))\n\n"
        ViewHelpers.contentForSet("title", "Articles")
        io += "\n<div class=\"w-full\">\n"
        if let notice = notice, !notice.isEmpty {
            io += "    <p class=\"py-2 px-3 bg-green-50 mb-5 text-green-500 font-medium rounded-md inline-block\" id=\"notice\">\(ViewHelpers.htmlEscape(notice))</p>\n"
        }
        io += "\n  <div class=\"flex justify-between items-center\">\n    <h1 class=\"font-bold text-4xl\">Articles</h1>\n    <a href=\"\(ViewHelpers.htmlEscape(RouteHelpers.newArticlePath()))\" class=\"rounded-md px-3.5 py-2.5 bg-blue-600 hover:bg-blue-500 text-white block font-medium\">New article</a>\n  </div>\n\n  <div id=\"articles\" class=\"min-w-full divide-y divide-gray-200 space-y-5\">\n"
        if !articles.isEmpty {
            io += "      "
            for a in articles {
                io += article(a)
            }
            io += "\n"
        } else {
            io += "      <p class=\"text-center my-10\">No articles found.</p>\n"
        }
        io += "  </div>\n</div>\n"
        return io
    }

    static func article(_ article: Article, _ notice: String? = nil, _ alert: String? = nil) -> String {
        var io = ""
        io += "<div id=\"\(ViewHelpers.domId(article))\" class=\"flex flex-col sm:flex-row justify-between items-center pb-5 sm:pb-0\">\n  <div class=\"p-4 border rounded mb-4 flex-grow\">\n    <h2 class=\"text-xl font-bold\">\n      <a href=\"\(ViewHelpers.htmlEscape(RouteHelpers.articlePath(article.id)))\" class=\"text-blue-600 hover:underline\">\(ViewHelpers.htmlEscape(article.title))</a>\n      <span id=\"\(ViewHelpers.domId(article, "comments_count"))\" class=\"text-gray-500 text-sm font-normal ml-2\">\n        (\(Inflector.pluralize(article.comments.count, "comment")))\n      </span>\n    </h2>\n    <p class=\"text-gray-700 mt-2\">\(ViewHelpers.htmlEscape(ViewHelpers.truncate(article.body, 100)))</p>\n  </div>\n  <div class=\"w-full sm:w-auto flex flex-col sm:flex-row space-x-2 space-y-2\">\n    <a href=\"\(ViewHelpers.htmlEscape(RouteHelpers.articlePath(article.id)))\" class=\"w-full sm:w-auto text-center rounded-md px-3.5 py-2.5 bg-gray-100 hover:bg-gray-50 inline-block font-medium\">Show</a>\n    <a href=\"\(ViewHelpers.htmlEscape(RouteHelpers.editArticlePath(article.id)))\" class=\"w-full sm:w-auto text-center rounded-md px-3.5 py-2.5 bg-gray-100 hover:bg-gray-50 inline-block font-medium\">Edit</a>\n    <form action=\"\(ViewHelpers.htmlEscape(RouteHelpers.articlePath(article.id)))\" method=\"post\" class=\"button_to\">\(ViewHelpers.methodOverrideInput("delete"))<button type=\"submit\" class=\"w-full sm:w-auto rounded-md px-3.5 py-2.5 text-white bg-red-600 hover:bg-red-500 font-medium cursor-pointer\" data-turbo-confirm=\"Are you sure?\">Destroy</button>\(ViewHelpers.csrfTokenHiddenInput())</form>\n  </div>\n</div>\n"
        return io
    }
}
