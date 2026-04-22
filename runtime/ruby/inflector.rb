module Inflector
  def pluralize(count, word)
    count == 1 ? "1 #{word}" : "#{count} #{word}s"
  end
end
