# CRuby override of image_tag: Rails' `size:` option expansion.
#
# The shared runtime's image_tag merges caller opts verbatim, which
# ships a literal `size="16x16"` attribute; Rails extracts :size and
# appends width/height. Attribute ORDER mirrors Rails' assembly
# (caller opts, then src, then width/height last) so avatar imgs
# byte-match the Rails render. Overlay, not shared: the delete-and-
# fan-out hash reshaping is lobsters-only surface (blog never passes
# size:), and untyped hash mutation is the shape the typed runtime
# refuses.
module ActionView
  module ViewHelpers
    def self.image_tag(source, opts = {})
      attrs = {}
      size = nil
      opts.to_h.each do |k, v|
        if k == :size
          size = v
        else
          attrs[k] = v
        end
      end
      attrs[:src] = image_path(source)
      if size
        w, h = size.to_s.split("x")
        attrs[:width] = w
        attrs[:height] = h || w
      end
      "<img#{render_attrs(attrs)}>"
    end
  end
end
