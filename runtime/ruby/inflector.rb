module Inflector
  def self.pluralize(count, word)
    count == 1 ? "1 #{word}" : "#{count} #{word}s"
  end

  PARAMETERIZE_SQUASH = /[^a-z0-9\-_]+/.freeze
  PARAMETERIZE_RUNS = /-{2,}/.freeze
  PARAMETERIZE_EDGES = /\A-|-\z/.freeze

  # AS `String#parameterize`, default separator: downcase, squash
  # non-alphanumerics to "-", collapse runs, trim edges. (No
  # transliteration pass — non-ASCII chars drop like Rails'
  # post-transliterate "?" placeholders do under gsub.) Separator-kwarg
  # call sites stay on the CRuby overlay's String reopen; only the
  # zero-arg form grounds here.
  def self.parameterize(str)
    str.downcase
       .gsub(PARAMETERIZE_SQUASH, "-")
       .gsub(PARAMETERIZE_RUNS, "-")
       .gsub(PARAMETERIZE_EDGES, "")
  end
end
