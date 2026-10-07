# CRuby-only spellings of three shared ViewHelpers hot paths.
#
# The shared bodies are written in the one shape every target emitter
# lowers (see the comments on `boolean_attr?` and `needs_url_escape?`
# in action_view/view_helpers.rb): `||` chains of `==` / `include?`, and
# `out = out + ...` concatenation. Under CRuby those are 45 comparisons
# per attribute, 32 scans per URL value, and a new String per attribute
# appended; every sidebar room link goes through all three. Same output.
module ActionView
  module ViewHelpers
    # Rails' BOOLEAN_ATTRIBUTES — the shared `boolean_attr?` chain, as a set.
    BOOLEAN_ATTRIBUTE_SET = %w[
      allowfullscreen allowpaymentrequest async autofocus autoplay checked
      compact controls declare default defaultchecked defaultmuted
      defaultselected defer disabled enabled formnovalidate hidden
      indeterminate inert ismap itemscope loop multiple muted nohref
      nomodule noresize noshade novalidate nowrap open pauseonexit
      playsinline pubdate readonly required reversed scoped seamless
      selected sortable truespeed typemustmatch visible
    ].to_h { |name| [name, true] }.freeze

    def self.boolean_attr?(name)
      BOOLEAN_ATTRIBUTE_SET.key?(name)
    end

    # One scan with the pattern the gsub that follows uses.
    def self.needs_url_escape?(s)
      s.match?(URL_ESCAPE_PATTERN)
    end

    def self.render_attrs(attrs)
      return "" if attrs.empty?
      out = +""
      attrs.each do |k, v|
        next if v.nil?
        name = k.to_s
        if v.is_a?(Hash)
          v.each do |inner_k, inner_v|
            next if inner_v.nil?
            out << " " << name << "-" << inner_k.to_s.tr("_", "-") << "=\"" << html_escape(inner_v.to_s) << "\""
          end
        elsif boolean_attr?(name)
          out << " " << name << "=\"" << name << "\"" unless v.to_s == "false"
        else
          out << " " << name << "=\"" << html_escape(attr_value_text(name, v)) << "\""
        end
      end
      out
    end
  end
end
