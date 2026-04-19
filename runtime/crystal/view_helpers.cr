# Roundhouse Crystal view helpers.
#
# Hand-written, shipped alongside generated code (copied in by the
# Crystal emitter as `src/view_helpers.cr`). Emitted view functions
# accumulate into `_buf : String` and call into these helpers for
# the Rails-shaped surface (link_to, form_with, turbo_stream_from,
# content_for).
#
# Degrade-gracefully stance: helpers return enough HTML that
# substring-matching controller tests pass (`<form`, `<a`, `<h1>`);
# real styling + attributes come in a later phase.

module Roundhouse
  module ViewHelpers
    # `<a href="...">label</a>` — minimal anchor tag. Extra options
    # land as HTML attributes (class, data-*).
    def self.link_to(label : String, url : String = "", opts : Hash(String, String) = {} of String => String) : String
      attrs = opts.map { |k, v| %( #{k}="#{v}") }.join
      %(<a href="#{url}"#{attrs}>#{label}</a>)
    end

    # `<button_to "Label", path, method: :delete>` — emits a form
    # wrapping a submit button so non-GET verbs work without JS.
    def self.button_to(label : String, url : String = "", opts : Hash(String, String) = {} of String => String) : String
      verb = opts["method"]? || "post"
      %(<form action="#{url}" method="#{verb}"><button type="submit">#{label}</button></form>)
    end

    # `turbo_stream_from "articles"` — the stream subscription tag.
    # Rendered as a bare `<turbo-cable-stream-source>` element.
    def self.turbo_stream_from(name : String) : String
      %(<turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="#{name}"></turbo-cable-stream-source>)
    end

    # `dom_id @article` → `"article_1"`. Uses the record's class
    # name (lowercased) and id.
    def self.dom_id(record) : String
      "#{record.class.name.downcase}_#{record.id}"
    end

    # `pluralize(n, "comment")` → `"1 comment"` / `"2 comments"`.
    def self.pluralize(count : Int, word : String) : String
      "#{count} #{count == 1 ? word : "#{word}s"}"
    end

    # `content_for(:title, "Articles")` — no-op in the compile-only
    # path; views with `<% content_for %>` blocks emit a call to this
    # and ignore the return value.
    def self.content_for(slot : Symbol | String, value : String = "") : String
      ""
    end

    # `form_with model: @article do |form| ... end` — wrap an inner
    # buffer in a `<form>` tag. The block is expected to return a
    # String built up from FormBuilder calls.
    def self.form_wrap(action : String?, css_class : String, inner : String) : String
      action_attr = action ? %( action="#{action}") : ""
      class_attr = css_class.empty? ? "" : %( class="#{css_class}")
      %(<form#{action_attr}#{class_attr}>#{inner}</form>)
    end

    # Minimal FormBuilder — enough methods to cover the scaffold
    # blog's _form partial (label, text_field, textarea, submit).
    class FormBuilder
      @record : String?

      def initialize(@record : String? = nil)
      end

      def label(field : Symbol | String, text : String = "") : String
        text = field.to_s.gsub('_', ' ').capitalize if text.empty?
        %(<label for="#{field}">#{text}</label>)
      end

      def text_field(field : Symbol | String, opts : Hash(String, String) = {} of String => String) : String
        attrs = opts.map { |k, v| %( #{k}="#{v}") }.join
        %(<input type="text" name="#{field}"#{attrs}>)
      end

      def textarea(field : Symbol | String, opts : Hash(String, String) = {} of String => String) : String
        attrs = opts.map { |k, v| %( #{k}="#{v}") }.join
        %(<textarea name="#{field}"#{attrs}></textarea>)
      end

      def submit(label : String = "Submit", opts : Hash(String, String) = {} of String => String) : String
        attrs = opts.map { |k, v| %( #{k}="#{v}") }.join
        %(<button type="submit"#{attrs}>#{label}</button>)
      end
    end
  end
end
