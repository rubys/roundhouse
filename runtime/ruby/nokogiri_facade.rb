# Nokogiri façade — its own file, so the spin-shaped spinel tree can swap
# the FILE for `require "nokogiri"` (the spinel-nokogiri spin package:
# libxml2 2.13.9 with Nokogiri's patches, carried) when the app names
# Nokogiri; see project::spin_shape. Targets without the package keep this
# raising stand-in, and spinel's read-path reopen (nokogiri_spinel.rb).
# Required from gem_facades.rb, after GemFacade is defined.

# Nokogiri — HTML parsing/DOM surgery (Markdowner post-processing,
# Story#fetched_attributes title extraction).
module Nokogiri
  def self.HTML(_html)
    GemFacade.fail!("Nokogiri.HTML")
    Document.new
  end

  # One attribute of an element (`el.attributes["content"]`).
  class Attr
    def text
      GemFacade.fail!("Nokogiri::Attr#text")
      ""
    end

    def to_s
      GemFacade.fail!("Nokogiri::Attr#to_s")
      ""
    end
  end

  class Element
    def name=(_value)
      GemFacade.fail!("Nokogiri::Element#name=")
      _value
    end

    def [](_key)
      GemFacade.fail!("Nokogiri::Element#[]")
      ""
    end

    def []=(_key, _value)
      GemFacade.fail!("Nokogiri::Element#[]=")
      _value
    end

    def text
      GemFacade.fail!("Nokogiri::Element#text")
      ""
    end

    def inner_html
      GemFacade.fail!("Nokogiri::Element#inner_html")
      ""
    end

    def attributes
      GemFacade.fail!("Nokogiri::Element#attributes")
      { "" => Attr.new }
    end

    def content=(_value)
      GemFacade.fail!("Nokogiri::Element#content=")
      _value
    end

    def replace(_node)
      GemFacade.fail!("Nokogiri::Element#replace")
      nil
    end
  end

  class NodeSet
    def each
      GemFacade.fail!("Nokogiri::NodeSet#each")
      yield Element.new
      nil
    end
  end

  class Document
    def css(_selector)
      GemFacade.fail!("Nokogiri::Document#css")
      NodeSet.new
    end

    # The encoding the document DECLARED (`<meta charset>` /
    # `<meta http-equiv="Content-Type">`), not the encoding it was
    # parsed as — nil when it declared none. The optionality is the
    # whole point of the member: campfire's
    # `Opengraph::Document#sanitize_content` uses it as a condition,
    # and a String-typed answer would fold that branch to always-taken.
    # `String?` therefore lives in gem_facades.rbs, the same split
    # `Element#[]` already uses (a `""` tail here, optional there).
    def meta_encoding
      GemFacade.fail!("Nokogiri::Document#meta_encoding")
      ""
    end

    def at_css(_selector)
      GemFacade.fail!("Nokogiri::Document#at_css")
      Element.new
    end

    def create_element(_name)
      GemFacade.fail!("Nokogiri::Document#create_element")
      Element.new
    end
  end
end
