module Inflector
  def self.pluralize(count, word)
    count == 1 ? "1 #{word}" : "#{count} #{word}s"
  end
end

# parameterize lives in the ruby-family reopen (inflector_ext.rb), NOT
# here: this file is transpiled into every strict target's runtime via
# the runtime_loader tables, and its gsub-with-Regexp bodies don't
# compile there (rust has no String#gsub, csharp routed it to the
# hash-replacement overload). Scaffold trees load the reopen through
# this require; the table transpilers collect methods only.
require_relative "inflector_ext"
