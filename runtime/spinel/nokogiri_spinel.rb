# Nokogiri, reopened for spinel: the READ path the façade in
# `runtime/gem_facades.rb` declines, for the one document shape an app
# in the corpus reads — campfire's `Opengraph::Document`, which asks a
# parsed page for its `<meta property="og:…">` / `<meta name="og:…">`
# tags and whether it declared an encoding.
#
# NOT NAMED `nokogiri.rb`: the ruby family loads the real gem
# (`project::GEM_REQUIRES`), and `walk_dir_flat` copies every
# runtime/spinel/*.rb into every tree, so under the gem's own name this
# would answer a bare `require "nokogiri"` there. Same rule as
# `cgi_spinel.rb`. Loaded from spinel's boot.rb only, after
# runtime/action_text — whose `Fragment` scanner does the tag walk and
# the attribute parse (quotes, entities), so this file is the XPath
# subset and the meta-charset rule and nothing else.
#
# WHAT IS ANSWERED, AND WHAT IS REFUSED. `Document#xpath` reads exactly
# the grammar campfire writes:
#
#     //meta[starts-with(@property, "og:") or starts-with(@name, "og:")]
#     //*/meta[...]                       (the same, one step deeper)
#     //meta                              (no predicate)
#
# an element name, optionally under `//*/`, with a predicate that is
# one or more `starts-with(@attr, "literal")` joined by `or`. Any other
# expression — a `contains`, an `and`, an axis, a positional index — is
# `GemFacade.fail!`, the façade's own loud refusal, rather than a silent
# empty NodeSet that would read as "the page has no such tags".
# `Element#[]`, `#key?` and `Document#meta_encoding` are the reads
# behind that one query; the façade's write-path members keep failing.
#
# `meta_encoding` is Nokogiri's: the `charset` attribute of a `<meta>`,
# else the `charset=` parameter of a `<meta http-equiv="Content-Type">`,
# else nil — which is what `Opengraph::Document#sanitize_content` tests
# to decide whether to strip non-UTF-8 bytes from a tag's content.
require_relative "gem_facades"
require_relative "action_text"

module Nokogiri
  def self.HTML(html)
    Document.new.parse(html.to_s)
  end

  class Element
    # The façade's `Element.new` takes no arguments (its failing tails
    # build one), so an element is filled after construction.
    def load(name, attributes)
      @name = name
      @attributes = attributes
      self
    end

    def name
      @name.to_s
    end

    def key?(key)
      @attributes.key?(key.to_s)
    end

    def [](key)
      @attributes.fetch(key.to_s, nil)
    end

    def attributes_hash
      @attributes
    end
  end

  class Document
    def parse(html)
      @html = html
      self
    end

    def xpath(expression)
      text = expression.to_s.strip
      text = text[2, text.length - 2].to_s if text.start_with?("//")
      text = text[2, text.length - 2].to_s if text.start_with?("*/")
      bracket = text.index("[")
      name = bracket.nil? ? text : text[0, bracket].to_s
      predicate = bracket.nil? ? "" : text[bracket + 1, text.length - bracket - 2].to_s
      unless ActionText::Fragment.element_name?(name) && (bracket.nil? || text.end_with?("]"))
        GemFacade.fail!("Nokogiri::Document#xpath(#{expression.inspect})")
      end
      keys = []
      prefixes = []
      predicate.split(" or ").each do |clause|
        clause = clause.strip
        next if clause == ""
        # starts-with(@attr, "prefix")
        unless clause.start_with?("starts-with(@") && clause.end_with?(")")
          GemFacade.fail!("Nokogiri::Document#xpath(#{expression.inspect})")
        end
        inner = clause[13, clause.length - 14].to_s
        comma = inner.index(",")
        GemFacade.fail!("Nokogiri::Document#xpath(#{expression.inspect})") if comma.nil?
        keys << inner[0, comma].to_s.strip
        prefixes << inner[comma + 1, inner.length].to_s.strip.gsub("\"", "").gsub("'", "")
      end
      out = []
      ActionText::Fragment.new(@html.to_s).find_all(name).each do |node|
        if Document.starts_with_any?(node.attributes, keys, prefixes)
          out << Element.new.load(node.name, node.attributes)
        end
      end
      out
    end

    # No predicate matches everything; otherwise any one clause does.
    def self.starts_with_any?(attributes, keys, prefixes)
      return true if keys.empty?
      i = 0
      while i < keys.length
        value = attributes.fetch(keys[i], nil)
        return true if !value.nil? && value.start_with?(prefixes[i])
        i = i + 1
      end
      false
    end

    def meta_encoding
      ActionText::Fragment.new(@html.to_s).find_all("meta").each do |node|
        charset = node["charset"]
        return charset if !charset.nil? && charset != ""
        equiv = node["http-equiv"]
        content = node["content"]
        if !equiv.nil? && equiv.downcase == "content-type" && !content.nil?
          at = content.downcase.index("charset=")
          if !at.nil?
            value = content[at + 8, content.length].to_s.strip
            semi = value.index(";")
            value = value[0, semi].to_s if !semi.nil?
            return value.gsub("\"", "").gsub("'", "").strip if value != ""
          end
        end
      end
      nil
    end
  end
end
