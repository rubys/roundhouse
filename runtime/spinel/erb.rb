# Spinel-only ERB shim. CRuby/JRuby get `ERB::Util` from the stdlib that
# Rails loads for them; the AOT tree has no stdlib ERB, so the constant
# resolved to nothing and `ERB::Util.html_escape` raised
# `undefined method 'html_escape' for unknown` (lobsters Hat#to_html_label,
# reached from every /u/:username render).
#
# Only `html_escape` is provided — the template half of ERB is not
# something an AOT tree can host (roundhouse compiles the templates), and
# `Util.url_encode` has no call site in the corpus. Add siblings when one
# appears rather than guessing at the surface.
#
# The escaping itself is delegated, not re-implemented: ActionView's
# `html_escape` is the same five-entity substitution, it is already the
# hot path every view goes through, and one implementation means one place
# to tune (see the CGI shim's note on `Url.escape`).
module ERB
  module Util
    # `to_s` is faithful, not a widening dodge: stdlib ERB::Util is
    # `CGI.escapeHTML(s.to_s)`, and the corpus leans on it — the two
    # lobsters call sites pass a nullable `link` column.
    def self.html_escape(s)
      ActionView::ViewHelpers.html_escape(s.to_s)
    end
  end
end
