# Same independent contract; Spinel uses its bundled native JSON package,
# not the installed CRuby JSON gem/C extension.
require "json"
require_relative "../sources/json"
require_relative "../contracts/json"
