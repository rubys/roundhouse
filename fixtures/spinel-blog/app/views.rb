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
