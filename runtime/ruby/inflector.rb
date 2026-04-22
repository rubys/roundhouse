module Inflector
  def self.pluralize(count, word)
    count == 1 ? "1 #{word}" : "#{count} #{word}s"
  end
end
