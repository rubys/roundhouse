# HtmlEncoder façade — lobsters' extras/html_encoder.rb wraps the
# `htmlentities` gem (`HTML_ENTITIES = HTMLEntities.new` at module
# scope, `HTML_ENTITIES.encode(string, type)`). The gem is a CRuby
# dependency the AOT tree has no bundle for, and unlike the Markdowner
# façade this one CANNOT raise: `HtmlEncoder.encode` is READ-path —
# home/rss.rbuilder encodes every story title, description, and comment
# link on the /rss feed — so the façade reimplements the encoding
# instead of standing in for it.
#
# WHAT THE GEM DOES, for the one configuration the app uses (default
# `xhtml1` flavor, `:decimal` instruction — see the gem's
# encoder.rb): emit `&#N;` for a character matching EITHER
#
#   the basic set  /[<>'"&]/            (xhtml1 keeps the apostrophe;
#                                        the html4 flavors move it to
#                                        the extended pattern instead)
#   the extended set  /[^\u{20}-\u{7E}]/  everything outside printable
#                                        ASCII — control characters and
#                                        newlines included, not just the
#                                        codepoints with named entities
#
# and pass every other character through verbatim. With `:decimal` and
# no `:named`/`:basic`, BOTH sets encode numerically, so the gem's
# entity-NAME tables never reach the output and this façade needs no
# copy of them — the rule is the two character sets above, nothing
# more. Checked against the installed gem over every valid codepoint
# 0..0x10FFFF: identical output for all 1,112,064.
#
# The CRuby/JRuby trees restore the verbatim gem-backed body over this
# (emit::ruby::library::restore_extras_facades), so the gem remains the
# authority wherever it is installed and this file only ever runs under
# the AOT binary.
require_relative "../../runtime/cgi"

module HtmlEncoder
  # `type` mirrors the gem's instruction argument. Only :decimal is
  # implemented — it is the app's default and its only call shape; a
  # caller asking for :named/:hexadecimal would need the name table
  # this façade deliberately doesn't carry.
  def self.encode(string, type = :decimal)
    out = +""
    string.to_s.each_char do |c|
      n = c.ord
      if entity?(n)
        out << "&#"
        out << n.to_s
        out << ";"
      else
        out << c
      end
    end
    out
  end

  # True for the characters the gem replaces: outside printable ASCII
  # (0x20..0x7E), or one of the five basic-set characters inside it.
  def self.entity?(n)
    return true if n < 32 || n > 126
    n == 34 || n == 38 || n == 39 || n == 60 || n == 62
  end

  def self.decode(encoded_string)
    CGI.unescape_html(encoded_string)
  end
end
