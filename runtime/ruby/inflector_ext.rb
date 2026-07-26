# Inflector reopen — ruby-family surface only (connection.rb pattern):
# shipped to the scaffold trees via the project.rs stems list and
# required from inflector.rb, but NOT in the runtime_loader tables, so
# the strict-target transpilers never see the gsub-with-Regexp bodies.
# Promote to inflector.rb only with per-target regex-replace support.
module Inflector
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

  # AS `String#pluralize(count)` — count-aware inflection of the string
  # itself: singular when `count == 1`, else the inflected plural (NOT the
  # count-labeling `pluralize(count, word)` in inflector.rb). Regex-free
  # (spinel-safe) rendition of the CRuby overlay's String#pluralize
  # rule-subset; the emit lowering grounds `"comment".pluralize(n)` here
  # because the built-in String can't be reopened on spinel. Lowercase
  # rules only (the corpus' inflected words are lowercase).
  def self.pluralize_word(word, count)
    return word if count == 1
    n = word.length
    return "s" if n == 0
    last = word[n - 1, 1]
    if last == "y" && n > 1
      prev = word[n - 2, 1]
      if prev == "a" || prev == "e" || prev == "i" || prev == "o" || prev == "u"
        word + "s"
      else
        word[0, n - 1] + "ies"
      end
    elsif last == "s"
      word
    elsif last == "x" || last == "z"
      word + "es"
    elsif n > 1 && (word[n - 2, 2] == "ch" || word[n - 2, 2] == "sh")
      word + "es"
    else
      word + "s"
    end
  end
end
