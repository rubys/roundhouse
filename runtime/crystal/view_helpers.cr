# Roundhouse Crystal view helpers.
#
# Hand-written, shipped alongside generated code (copied in by the
# Crystal emitter as `src/view_helpers.cr`). Ports the rust/TS/
# python/go helper surface: link_to, button_to, FormBuilder,
# turbo_stream_from, dom_id, pluralize, truncate, layout-slot
# storage for `yield` / `content_for`.

require "base64"
require "html"
require "json"

module Roundhouse
  module ViewHelpers
    # ── Render state (yield + content_for slots) ───────────────

    @@yield_body : String = ""
    @@slots : Hash(String, String) = {} of String => String

    def self.reset_render_state : Nil
      @@yield_body = ""
      @@slots = {} of String => String
    end

    def self.set_yield(body : String) : Nil
      @@yield_body = body
    end

    def self.get_yield : String
      @@yield_body
    end

    def self.get_slot(name : String) : String
      @@slots[name]? || ""
    end

    def self.content_for_set(slot : String, body : String) : Nil
      @@slots[slot] = body
    end

    def self.content_for_get(slot : String) : String
      @@slots[slot]? || ""
    end

    # ── Layout-meta helpers ────────────────────────────────────

    def self.csrf_meta_tags : String
      %(<meta name="csrf-param" content="authenticity_token" />\n<meta name="csrf-token" content="" />)
    end

    def self.csp_meta_tag : String
      ""
    end

    def self.stylesheet_link_tag(name : String, opts : Hash(String, String) = {} of String => String) : String
      href = "/assets/#{name}.css"
      attrs = sorted_attrs(opts)
      %(<link rel="stylesheet" href="#{HTML.escape(href)}"#{attrs} />)
    end

    def self.javascript_importmap_tags(pins : Array(Tuple(String, String)), main_entry : String = "application") : String
      String.build do |io|
        io << %(<script type="importmap" data-turbo-track="reload">{\n)
        io << %(  "imports": {\n)
        pins.each_with_index do |(name, path), i|
          sep = i + 1 < pins.size ? "," : ""
          io << "    #{name.to_json}: #{path.to_json}#{sep}\n"
        end
        io << "  }\n"
        io << "}</script>"
        pins.each do |(_, path)|
          io << "\n"
          io << %(<link rel="modulepreload" href="#{HTML.escape(path)}">)
        end
        io << "\n"
        io << %(<script type="module">import "#{HTML.escape(main_entry)}"</script>)
      end
    end

    # ── link_to / button_to ────────────────────────────────────

    def self.link_to(text : String, url : String, opts : Hash(String, String) = {} of String => String) : String
      %(<a href="#{HTML.escape(url)}"#{sorted_attrs(opts)}>#{HTML.escape(text)}</a>)
    end

    def self.button_to(text : String, target : String, opts : Hash(String, String) = {} of String => String) : String
      method = opts["method"]? || "post"
      button_class = opts["class"]? || ""
      form_class = opts["form_class"]? || "button_to"
      method_lower = method.downcase
      method_input = ""
      if method_lower != "post" && method_lower != "get"
        method_input = %(<input type="hidden" name="_method" value="#{HTML.escape(method)}" />)
      end
      button_attrs = String.build do |io|
        opts.keys.sort.each do |k|
          next unless k.starts_with?("data-")
          io << %( #{HTML.escape(k)}="#{HTML.escape(opts[k])}")
        end
      end
      button_cls_attr = button_class.empty? ? "" : %( class="#{HTML.escape(button_class)}")
      csrf_input = %(<input type="hidden" name="authenticity_token" value="">)
      %(<form class="#{HTML.escape(form_class)}" method="post" action="#{HTML.escape(target)}">#{method_input}<button#{button_cls_attr}#{button_attrs} type="submit">#{HTML.escape(text)}</button>#{csrf_input}</form>)
    end

    # ── form_with wrapper ──────────────────────────────────────

    def self.form_wrap(action : String, is_persisted : Bool, html_class : String, inner : String) : String
      class_attr = html_class.empty? ? "" : %( class="#{HTML.escape(html_class)}")
      method_input = is_persisted ? %(<input type="hidden" name="_method" value="patch">) : ""
      csrf_input = %(<input type="hidden" name="authenticity_token" value="">)
      %(<form#{class_attr} action="#{HTML.escape(action)}" accept-charset="UTF-8" method="post">#{method_input}#{csrf_input}#{inner}</form>)
    end

    # ── FormBuilder ────────────────────────────────────────────

    class FormBuilder
      property prefix : String
      property css_class : String
      property is_persisted : Bool

      def initialize(@prefix : String = "", @css_class : String = "", @is_persisted : Bool = false)
      end

      def name_for(field : String) : String
        prefix.empty? ? field : "#{prefix}[#{field}]"
      end

      def id_for(field : String) : String
        prefix.empty? ? field : "#{prefix}_#{field}"
      end

      def label(field : String, opts : Hash(String, String) = {} of String => String) : String
        cls = opts["class"]?
        class_attr = (cls && !cls.empty?) ? %( class="#{HTML.escape(cls)}") : ""
        text = field.empty? ? field : (field[0].upcase.to_s + field[1..])
        %(<label for="#{HTML.escape(id_for(field))}"#{class_attr}>#{HTML.escape(text)}</label>)
      end

      def text_field(field : String, value : String = "", opts : Hash(String, String) = {} of String => String) : String
        cls = opts["class"]?
        class_attr = (cls && !cls.empty?) ? %( class="#{HTML.escape(cls)}") : ""
        value_attr = value.empty? ? "" : %( value="#{HTML.escape(value)}")
        %(<input type="text" name="#{HTML.escape(name_for(field))}" id="#{HTML.escape(id_for(field))}"#{value_attr}#{class_attr} />)
      end

      def textarea(field : String, value : String = "", opts : Hash(String, String) = {} of String => String) : String
        cls = opts["class"]?
        class_attr = (cls && !cls.empty?) ? %( class="#{HTML.escape(cls)}") : ""
        rows = opts["rows"]?
        rows_attr = (rows && !rows.empty?) ? %( rows="#{HTML.escape(rows)}") : ""
        body = value.empty? ? "" : HTML.escape(value)
        %(<textarea#{rows_attr}#{class_attr} name="#{HTML.escape(name_for(field))}" id="#{HTML.escape(id_for(field))}">\n#{body}</textarea>)
      end

      def submit(opts : Hash(String, String) = {} of String => String) : String
        cls = opts["class"]?
        class_attr = (cls && !cls.empty?) ? %( class="#{HTML.escape(cls)}") : ""
        label = opts["label"]?
        if label.nil? || label.empty?
          prefix_human = prefix.empty? ? "" : prefix[0].upcase.to_s + prefix[1..]
          label = is_persisted ? "Update #{prefix_human}" : "Create #{prefix_human}"
        end
        esc = HTML.escape(label)
        %(<input type="submit" name="commit" value="#{esc}"#{class_attr} data-disable-with="#{esc}" />)
      end
    end

    # ── Turbo / misc ───────────────────────────────────────────

    def self.turbo_stream_from(channel : String) : String
      encoded = Base64.strict_encode(channel.to_json)
      %(<turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="#{encoded}--unsigned"></turbo-cable-stream-source>)
    end

    def self.dom_id(singular : String, id : Int, prefix : String = "") : String
      base = "#{singular}_#{id}"
      prefix.empty? ? base : "#{prefix}_#{base}"
    end

    def self.pluralize(count : Int64, word : String) : String
      count == 1 ? "1 #{word}" : "#{count} #{word}s"
    end

    def self.truncate(text : String, opts : Hash(String, String) = {} of String => String) : String
      length = (opts["length"]? || "30").to_i
      omission = opts["omission"]? || "..."
      return text if text.size <= length
      cut = [length - omission.size, 0].max
      text[0, cut] + omission
    end

    def self.field_has_error(errors, field : String) : Bool
      errors.any? { |e| e.field == field }
    end

    def self.error_messages_for(errors, noun : String) : String
      _ = noun
      ""
    end

    def self.content_for(slot, body = nil)
      case body
      when String
        content_for_set(slot.to_s, body)
        ""
      else
        content_for_get(slot.to_s)
      end
    end

    # ── helpers ────────────────────────────────────────────────

    private def self.sorted_attrs(opts : Hash(String, String)) : String
      return "" if opts.empty?
      String.build do |io|
        opts.keys.sort.each do |k|
          io << %( #{HTML.escape(k)}="#{HTML.escape(opts[k])}")
        end
      end
    end
  end
end
