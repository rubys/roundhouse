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
end
