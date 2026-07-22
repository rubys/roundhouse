# Gem façades — typed, loudly-raising stand-ins for native/third-party
# gems that are REACHABLE in the require graph but never EXECUTED on
# the read path. Spinel AOT must type-check the whole reachable graph,
# so these classes exist to compile; every body fails loudly so an
# unexpected runtime hit raises NotImplementedError instead of silently
# misbehaving. Fill in real implementations (SpinelGems mirror or
# pure-Ruby replacement) per the gem-fate taxonomy when the write paths
# come into scope.
#
# Current occupants and why they're safe to stub:
# - Markly + Nokogiri: Markdowner and Story#fetched_attributes — both
#   write-path (markeddown_* columns are precomputed in the DB; URL
#   fetching happens at submit time). The CRuby tree ships no such
#   gems either and serves the full read benchmark.
# - Mail::Address: Hat#sanitized_link display helper, off the
#   benchmark's route set.
#
# Body shape: `GemFacade.fail!(...)` then an UNREACHABLE typed tail.
# The tail is never executed (fail! raises first) but it is what makes
# each member's return type inferable — a raise-only body infers as
# void under AOT, and then chained calls on the result (`.css(...)
# .each`) stop compiling. The helper call (rather than a bare `raise`)
# keeps the tail statically live.
module GemFacade
  def self.fail!(member)
    raise NotImplementedError,
          "gem facade: #{member} is stubbed (write-path only; see runtime/gem_facades.rb)"
  end
end

# Markly — CommonMark rendering (lobsters' Markdowner).
module Markly
  SMART = 0
  DEFAULT = 0

  def self.parse(_text, flags: 0, extensions: [])
    GemFacade.fail!("Markly.parse")
    Node.new
  end

  class Node
    def initialize(_type = nil)
      GemFacade.fail!("Markly::Node.new")
      @type = _type
    end

    def type
      GemFacade.fail!("Markly::Node#type")
      :text
    end

    def each
      GemFacade.fail!("Markly::Node#each")
      yield Node.new(:text)
      nil
    end

    def to_html(flags: 0, extensions: [])
      GemFacade.fail!("Markly::Node#to_html")
      ""
    end

    def string_content
      GemFacade.fail!("Markly::Node#string_content")
      ""
    end

    def string_content=(_value)
      GemFacade.fail!("Markly::Node#string_content=")
      _value
    end

    def url=(_value)
      GemFacade.fail!("Markly::Node#url=")
      _value
    end

    def insert_after(_node)
      GemFacade.fail!("Markly::Node#insert_after")
      nil
    end

    def append_child(_node)
      GemFacade.fail!("Markly::Node#append_child")
      nil
    end
  end
end

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

# mail — RFC822 address parsing (Hat#sanitized_link) and inbound message
# parsing (extras/email_parser.rb, the mailbox path — off the read
# benchmark).
module Mail
  def self.read_from_string(_text)
    GemFacade.fail!("Mail.read_from_string")
    Message.new
  end

  # One MIME part of a multipart message; parts nest (parts.first.parts).
  class Part
    def parts
      GemFacade.fail!("Mail::Part#parts")
      [Part.new]
    end

    def content_type
      GemFacade.fail!("Mail::Part#content_type")
      ""
    end

    def content_type_parameters
      GemFacade.fail!("Mail::Part#content_type_parameters")
      { "" => "" }
    end

    def body
      GemFacade.fail!("Mail::Part#body")
      ""
    end
  end

  class Message
    def multipart?
      GemFacade.fail!("Mail::Message#multipart?")
      false
    end

    def parts
      GemFacade.fail!("Mail::Message#parts")
      [Part.new]
    end

    def content_type
      GemFacade.fail!("Mail::Message#content_type")
      ""
    end

    def content_type_parameters
      GemFacade.fail!("Mail::Message#content_type_parameters")
      { "" => "" }
    end

    def body
      GemFacade.fail!("Mail::Message#body")
      ""
    end
  end

  class Address
    def initialize(_value)
      GemFacade.fail!("Mail::Address.new")
      @value = _value
    end

    def address
      GemFacade.fail!("Mail::Address#address")
      ""
    end

    def domain
      GemFacade.fail!("Mail::Address#domain")
      ""
    end

    def local
      GemFacade.fail!("Mail::Address#local")
      ""
    end
  end
end

# ROTP — TOTP two-factor auth (User#authenticate_totp, settings 2FA
# enrollment/verify). Off the read benchmark (all uses are write-path
# POSTs). `verify` returns Bool so the `if user.authenticate_totp(...)`
# condition type-checks under AOT.
module ROTP
  class TOTP
    def initialize(_secret, issuer: nil)
      GemFacade.fail!("ROTP::TOTP.new")
      @secret = _secret
    end

    def verify(_code)
      GemFacade.fail!("ROTP::TOTP#verify")
      false
    end

    def provisioning_uri(_account)
      GemFacade.fail!("ROTP::TOTP#provisioning_uri")
      ""
    end

    def secret
      GemFacade.fail!("ROTP::TOTP#secret")
      ""
    end
  end

  module Base32
    def self.random
      GemFacade.fail!("ROTP::Base32.random")
      ""
    end
  end
end

# BCrypt façade lives in its own file — the spin-shaped spinel tree
# swaps that FILE for `require "bcrypt"` (the spin package) when the
# app consumes BCrypt; whole-file grain, same as every other swap.
require_relative "bcrypt_facade"

# RQRCode — QR-code rendering for 2FA enrollment. Off the read benchmark.
module RQRCode
  class QRCode
    def initialize(_data)
      GemFacade.fail!("RQRCode::QRCode.new")
      @data = _data
    end

    def as_svg(offset: 0, fill: nil, color: nil, module_size: nil, shape_rendering: nil)
      GemFacade.fail!("RQRCode::QRCode#as_svg")
      ""
    end
  end
end

# SVG::Graph::TimeSeries — SVG statistics graphs. lib/time_series.rb
# subclasses it and the stats controller instantiates it; off the read
# benchmark. The app subclass calls these inherited methods, so they
# carry concrete return types (String format strings, Integer value
# arrays) rather than raising to void.
module SVG
  module Graph
    class TimeSeries
      def initialize(_opts)
        GemFacade.fail!("SVG::Graph::TimeSeries.new")
        @opts = _opts
      end

      def popup_format
        GemFacade.fail!("SVG::Graph::TimeSeries#popup_format")
        ""
      end

      def x_label_format
        GemFacade.fail!("SVG::Graph::TimeSeries#x_label_format")
        ""
      end

      def get_x_values
        GemFacade.fail!("SVG::Graph::TimeSeries#get_x_values")
        [0]
      end

      def get_y_values
        GemFacade.fail!("SVG::Graph::TimeSeries#get_y_values")
        [0]
      end

      def add_data(data: nil, template: nil)
        GemFacade.fail!("SVG::Graph::TimeSeries#add_data")
        self
      end

      def burn_svg_only
        GemFacade.fail!("SVG::Graph::TimeSeries#burn_svg_only")
        ""
      end
    end
  end
end
