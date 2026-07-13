# runtime/markly_jruby.rb — JRuby implementation of the markly contract
# over commonmark-java.
#
# The markly gem is cmark-gfm C bindings with no JRuby build, so the
# JRuby target provides the same surface via Java interop (the same move
# db_jruby.rb makes for SQLite over JDBC). The contract is the subset of
# markly that app code (lobsters' Markdowner) actually uses — typed in
# runtime/gem_facades.rbs, behaviorally pinned by
# scripts/markly-conformance, whose vectors are generated from the real
# gem running under CRuby (the reference implementation).
#
# Reference behaviors matched deliberately (see the vectors):
# - Raw HTML is omitted as `<!-- raw HTML omitted -->` (cmark's default
#   no-UNSAFE policy; makes the :tagfilter extension a no-op here).
# - SMART punctuation is applied at PARSE time on Text literals, so
#   node-surgery callers (Markdowner's @mention linkifier) see smarted
#   text — but NOT inside autolink-produced links (destination == text
#   in the AST; percent-encoding happens at render).
# - hrefs are percent-encoded at render (percentEncodeUrls).
#
# Known, accepted divergence: backslash-escaped quotes (\") are smarted
# here but stay straight under cmark — the escape is consumed by parse,
# so post-parse text can't distinguish them. Ledger in the vectors file
# if a real input ever hits it.
#
# Jars: commonmark, commonmark-ext-gfm-strikethrough,
# commonmark-ext-autolink, autolink (org.nibor). Resolved from
# $MARKLY_JRUBY_JARS, else <tree>/vendor/jars (populate with
# bin/fetch-jars), else ~/.roundhouse/jars.

raise LoadError, "markly_jruby.rb only runs under JRuby" unless RUBY_ENGINE == "jruby"

require "java"

jars_dir = [
  ENV["MARKLY_JRUBY_JARS"],
  File.expand_path("../vendor/jars", __dir__),
  File.expand_path("~/.roundhouse/jars"),
].compact.find { |d| !Dir.glob(File.join(d, "commonmark-*.jar")).empty? }

unless jars_dir
  raise LoadError,
        "markly_jruby: commonmark-java jars not found (run bin/fetch-jars, " \
        "or set MARKLY_JRUBY_JARS)"
end

Dir.glob(File.join(jars_dir, "*.jar")).sort.each { |jar| require jar }

module Markly
  JRUBY_SHIM = true
  DEFAULT = 0
  SMART = 1 << 10

  J = Java::OrgCommonmarkNode

  EXTENSION_MAP = {
    autolink: -> { Java::OrgCommonmarkExtAutolink::AutolinkExtension.create },
    strikethrough: -> { Java::OrgCommonmarkExtGfmStrikethrough::StrikethroughExtension.create },
    tagfilter: nil, # raw HTML is omitted entirely, nothing left to filter
  }.freeze

  def self.java_extensions(extensions)
    exts = java.util.ArrayList.new
    extensions.each do |sym|
      factory = EXTENSION_MAP.fetch(sym) do
        raise ArgumentError, "markly_jruby: unsupported extension #{sym.inspect}"
      end
      exts.add(factory.call) if factory
    end
    exts
  end

  # cmark html.c renders raw HTML as a comment unless UNSAFE is set;
  # markly's to_html(flags: DEFAULT) inherits that policy, so the shim's
  # renderer replaces HtmlInline/HtmlBlock output the same way.
  class RawHtmlOmitter
    include Java::OrgCommonmarkRenderer::NodeRenderer

    OMITTED = "<!-- raw HTML omitted -->"

    def initialize(context)
      @writer = context.get_writer
    end

    def get_node_types
      java.util.HashSet.new([J::HtmlInline.java_class, J::HtmlBlock.java_class])
    end

    def render(node)
      if node.is_a?(J::HtmlBlock)
        @writer.line
        @writer.raw(OMITTED)
        @writer.line
      else
        @writer.raw(OMITTED)
      end
    end
  end

  def self.parse(text, flags: DEFAULT, extensions: [])
    exts = java_extensions(extensions)
    parser = Java::OrgCommonmarkParser::Parser.builder.extensions(exts).build
    root = parser.parse(text.to_s)
    smart_punctuate!(root) if (flags & SMART) != 0
    Node.wrap(root)
  end

  def self.render_html(jnode, extensions)
    Java::OrgCommonmarkRendererHtml::HtmlRenderer.builder
      .extensions(java_extensions(extensions))
      .percent_encode_urls(true)
      .node_renderer_factory { |ctx| RawHtmlOmitter.new(ctx) }
      .build
      .render(jnode)
  end

  # ── SMART punctuation (cmark smart.c behavior, post-parse) ─────────
  # Walks Text literals in document order, carrying the previous
  # rendered character across nodes so open/close quote decisions match
  # cmark's source-order lexing. Code spans/blocks are distinct node
  # types (never smarted); autolinked Link subtrees are skipped, as
  # observed from the reference gem.

  OPENERS = " \t\n([{<“‘\"'-–—".freeze

  def self.open_quote?(prev)
    prev.nil? || OPENERS.include?(prev)
  end

  # cmark's hyphen-run rule: runs of n hyphens become em/en dashes.
  def self.dashes(run)
    if run % 3 == 0
      "—" * (run / 3)
    elsif run % 2 == 0
      "–" * (run / 2)
    elsif run % 3 == 2
      ("—" * ((run - 2) / 3)) + "–"
    else
      ("—" * ((run - 4) / 3)) + "––"
    end
  end

  def self.smart_text(str, state)
    out = +""
    prev = state[:prev]
    i = 0
    while i < str.length
      c = str[i]
      case c
      when "-"
        run = 1
        run += 1 while str[i + run] == "-"
        out << (run == 1 ? "-" : dashes(run))
        i += run
      when "."
        if str[i, 3] == "..."
          out << "…"
          i += 3
        else
          out << "."
          i += 1
        end
      when '"'
        out << (open_quote?(prev) ? "“" : "”")
        i += 1
      when "'"
        out << (open_quote?(prev) ? "‘" : "’")
        i += 1
      else
        out << c
        i += 1
      end
      prev = out[-1]
    end
    state[:prev] = prev
    out
  end

  def self.autolinked?(jlink)
    child = jlink.first_child
    return false unless child.is_a?(J::Text) && child.next.nil?

    text = child.literal
    dest = jlink.destination
    dest == text || dest == "http://#{text}" || dest == "mailto:#{text}"
  end

  def self.smart_punctuate!(jnode, state = { prev: nil })
    case jnode
    when J::Text
      jnode.literal = smart_text(jnode.literal, state)
      return
    when J::Code
      lit = jnode.literal
      state[:prev] = lit[-1] unless lit.nil? || lit.empty?
      return
    when J::SoftLineBreak, J::HardLineBreak
      state[:prev] = "\n"
      return
    when J::Link
      if autolinked?(jnode)
        text = jnode.first_child.literal
        state[:prev] = text[-1] unless text.empty?
        return
      end
    end

    child = jnode.first_child
    while child
      nxt = child.next
      smart_punctuate!(child, state)
      child = nxt
    end
  end

  # ── the node wrapper (markly's Node surface) ───────────────────────
  class Node
    TYPE_MAP = {
      "Document" => :document, "Paragraph" => :paragraph, "Text" => :text,
      "Link" => :link, "Image" => :image, "Heading" => :header,
      "Emphasis" => :emph, "StrongEmphasis" => :strong, "Code" => :code,
      "FencedCodeBlock" => :code_block, "IndentedCodeBlock" => :code_block,
      "BlockQuote" => :blockquote, "BulletList" => :list,
      "OrderedList" => :list, "ListItem" => :list_item,
      "HtmlBlock" => :html, "HtmlInline" => :inline_html,
      "SoftLineBreak" => :softbreak, "HardLineBreak" => :linebreak,
      "ThematicBreak" => :hrule, "Strikethrough" => :strikethrough,
    }.freeze

    attr_reader :j

    def self.wrap(jnode)
      node = allocate
      node.instance_variable_set(:@j, jnode)
      node
    end

    def initialize(type)
      @j = case type
           when :link then J::Link.new
           when :text then J::Text.new("")
           else raise ArgumentError, "markly_jruby: Node.new(#{type.inspect}) unsupported"
           end
    end

    def type
      TYPE_MAP.fetch(@j.java_class.simple_name) { @j.java_class.simple_name.downcase.to_sym }
    end

    # Live-pointer child iteration (matches markly: siblings inserted
    # during the walk are visited).
    def each
      child = @j.first_child
      while child
        yield Node.wrap(child)
        child = child.next
      end
    end

    def to_html(flags: DEFAULT, extensions: [])
      Markly.render_html(@j, extensions)
    end

    def string_content
      @j.literal
    end

    def string_content=(value)
      @j.literal = value
    end

    def url=(value)
      @j.destination = value
    end

    def url
      @j.destination
    end

    def insert_after(node)
      @j.insert_after(node.j)
    end

    def append_child(node)
      @j.append_child(node.j)
    end
  end
end
