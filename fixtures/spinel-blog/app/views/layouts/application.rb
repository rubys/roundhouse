require "action_view"

module Views
  module Layouts
    module_function

    # Lowered shape of fixtures/real-blog/app/views/layouts/application.html.erb.
    # Takes the rendered body string explicitly (the lowerer wraps each
    # action's view in a layout call). `content_for(:title)` slot is
    # populated by the inner view before this method runs, so reading
    # via `ViewHelpers.content_for_get(:title)` here yields whatever the
    # inner view set.
    def application(body)
      io = String.new
      title = ViewHelpers.content_for_get(:title) || "Real Blog"
      io << "<!DOCTYPE html>\n<html>\n  <head>\n"
      io << "    <title>"
      io << ViewHelpers.html_escape(title)
      io << "</title>\n"
      io << %(    <meta name="viewport" content="width=device-width,initial-scale=1">\n)
      io << %(    <meta name="apple-mobile-web-app-capable" content="yes">\n)
      io << %(    <meta name="application-name" content="Real Blog">\n)
      io << %(    <meta name="mobile-web-app-capable" content="yes">\n    )
      io << ViewHelpers.csrf_meta_tags
      io << "\n    "
      io << ViewHelpers.csp_meta_tag
      io << "\n\n    "
      io << ViewHelpers.get_slot(:head)
      io << "\n\n"
      io << %(    <link rel="icon" href="/icon.png" type="image/png">\n)
      io << %(    <link rel="icon" href="/icon.svg" type="image/svg+xml">\n)
      io << %(    <link rel="apple-touch-icon" href="/icon.png">\n\n    )
      io << ViewHelpers.stylesheet_link_tag("app", "data-turbo-track" => "reload")
      io << "\n    "
      io << ViewHelpers.javascript_importmap_tags
      io << "\n  </head>\n\n  <body>\n"
      io << %(    <main class="container mx-auto mt-28 px-5 flex flex-col">\n      )
      io << body
      io << "\n    </main>\n  </body>\n</html>\n"
      io
    end
  end
end
