# Loads all view files into the Views::* namespace. Files are loaded
# in dependency-friendly order (partials before the templates that use
# them), but each file's `require_relative` already pulls its own
# dependencies, so order here is mostly for clarity.

require_relative "views/comments/_comment"
require_relative "views/articles/_article"
require_relative "views/articles/_form"
require_relative "views/articles/index"
require_relative "views/articles/show"
require_relative "views/articles/new"
require_relative "views/articles/edit"
require_relative "views/layouts/application"

# Jbuilder-lowered renderers (Phase-3 jbuilder_to_library output).
# Each `_json.rb` reopens the same `Views::<Plural>` module its html
# sibling defines, adding `<base>_json(arg)` methods that return a
# JSON string. The controller side dispatches to these via Phase-8
# `format.json { render :show }` plumbing (in flight).
require_relative "views/articles/_article_json"
require_relative "views/articles/index_json"
require_relative "views/articles/show_json"
