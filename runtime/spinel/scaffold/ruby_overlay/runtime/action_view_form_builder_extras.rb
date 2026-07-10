# CRuby-only value-dependent FormBuilder attribute helpers.
#
# The form_builder macro-inline emits static HTML, but three controls
# carry attributes that depend on the record's CURRENT value —
# check_box's checked, radio_button's checked, select's selected
# option. These resolve here at request time. Overlay, not shared
# runtime: untyped-value truthiness and pair-array walking are the
# shapes the typed runtime refuses; strict targets get their own when
# lobsters reaches them.
module ActionView
  module ViewHelpers
    # ` checked="checked"` when the value is truthy and not 0/"0"/"f"
    # (Rails' checked_value comparison for the default 0/1 box).
    def self.checked_box_attr(value)
      on = value && value != 0 && value != "0" && value != false
      on ? " checked=\"checked\"" : ""
    end

    # ` checked="checked"` when the record's value stringifies equal to
    # the radio's value (Rails compares on to_s).
    def self.radio_checked_attr(current, value)
      current.to_s == value.to_s ? " checked=\"checked\"" : ""
    end

    # `<label for="name">Text</label>` — the bare (builder-less) tag
    # helper. Text defaults to the humanized name.
    def self.label_tag(name = nil, content = nil, _options = {})
      for_attr = name ? " for=\"#{html_escape(name.to_s)}\"" : ""
      text = content || (name ? name.to_s.tr("_", " ").capitalize : "")
      "<label#{for_attr}>#{html_escape(text.to_s)}</label>"
    end

    # `<input type="submit" name="commit" value="X" data-disable-with="X"
    # <opts>>` — the bare (builder-less) submit. `data: {…}` fans out to
    # data-* attributes; nil values drop.
    def self.submit_tag(value = "Save changes", options = {})
      attrs = +""
      options.each do |k, v|
        if k == :data && v.is_a?(Hash)
          v.each do |dk, dv|
            attrs << " data-#{dk}=\"#{html_escape(dv.to_s)}\"" unless dv.nil?
          end
        elsif !v.nil?
          attrs << " #{k}=\"#{html_escape(v.to_s)}\""
        end
      end
      "<input type=\"submit\" name=\"commit\" value=\"#{html_escape(value.to_s)}\"" \
        " data-disable-with=\"#{html_escape(value.to_s)}\"#{attrs}>"
    end

    # `<form action="URL" accept-charset="UTF-8" method="post"<opts>>` +
    # block content + `</form>` — the bare form_tag. The walker lowers
    # the block to a capture that returns its accumulated string.
    def self.form_tag(url, options = {})
      attrs = +""
      options.each do |k, v|
        attrs << " #{k}=\"#{html_escape(v.to_s)}\"" unless v.nil?
      end
      body = block_given? ? yield.to_s : ""
      "<form action=\"#{html_escape(url_for(url))}\" accept-charset=\"UTF-8\" method=\"post\"#{attrs}>#{body}</form>"
    end

    # `<option value="0">No e-mails</option>…` from `[[label, value],
    # …]` (or a flat list of strings), selecting the entry whose value
    # stringifies equal to the record's current value.
    def self.select_options_for(choices, current)
      out = +""
      choices.each do |choice|
        label, value = choice.is_a?(Array) ? [choice[0], choice[1]] : [choice, choice]
        selected = current.to_s == value.to_s ? " selected=\"selected\"" : ""
        out << "<option#{selected} value=\"#{html_escape(value.to_s)}\">#{html_escape(label.to_s)}</option>"
      end
      out
    end
  end
end
