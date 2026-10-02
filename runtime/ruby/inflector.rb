module Inflector
  def self.pluralize(count, word)
    count == 1 ? "1 #{word}" : "#{count} #{word}s"
  end

  # ActionView accepts a formatted count, preserving the label verbatim.
  # Separate from the Integer entry point so strict targets keep concrete
  # arguments. Rails' /^1(\.0+)?$/ singular check is line-anchored: even a
  # count containing a newline can have a matching line. The noun rules
  # remain the same regular-suffix subset as the Integer helper above.
  def self.pluralize_formatted(count, word)
    # Removing zeros only after checking the prefix and a nonempty suffix
    # distinguishes 1.00 from 1., 10.0 and 1.01, without regexes or mutable
    # loops (which are not supported by every target).
    singular_lines = count.split("\n").map do |line|
      line == "1" || (line.start_with?("1.") && line.length > 2 && line.split("0").join("") == "1.")
    end
    singular_lines.include?(true) ? "#{count} #{word}" : "#{count} #{word}s"
  end
end

# parameterize lives in the ruby-family reopen (inflector_ext.rb), NOT
# here: this file is transpiled into every strict target's runtime via
# the runtime_loader tables, and its gsub-with-Regexp bodies don't
# compile there (rust has no String#gsub, csharp routed it to the
# hash-replacement overload). Scaffold trees load the reopen through
# this require; the table transpilers collect methods only.
require_relative "inflector_ext"
