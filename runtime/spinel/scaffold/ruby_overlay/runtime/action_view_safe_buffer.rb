# Safe-buffer-aware overrides of the shared runtime's escape surface.
#
# Must load AFTER runtime/action_controller (whose require chain pulls
# in the shared runtime/action_view/view_helpers.rb) so these
# definitions win the module reopen — same ordering contract as the
# form_authenticity_token override in action_controller_session.rb.
#
# `html_escape` honoring `html_safe?` is what lets safety cross a VALUE
# boundary the emit-time unwrap can't see. Two live shapes from the
# lobsters byte-parity audit:
#
#   - layout: `link_to_different_page(raw("user&nbsp;<span…>"), path)`
#     — the safe label rides a plain parameter into the shared
#     `link_to`, whose `html_escape(text)` lands here and passes the
#     marked string through, exactly Rails' SafeBuffer behavior.
#   - `<%= hat.to_html_label %>` — an app-MODEL method that builds
#     markup and returns `h.html_safe`; the walker's default
#     `html_escape(<call>.to_s)` wrap stays, and the mark defuses it.
#
# The emit-time unwrap (is_html_safe_call in emit/ruby/library.rs)
# still strips the wrapper for every statically visible producer; this
# override covers the dynamic residue. CRuby-only: strict targets get
# a typed safe-string story when lobsters reaches them.
module ActionView
  module ViewHelpers
    def self.html_escape(s)
      return s if s.html_safe?
      s.gsub(HTML_ESCAPE_PATTERN, HTML_ESCAPES)
    end

    # `raw(x)` marks — it must return a SafeString (the shared default
    # returns a plain `to_s`) so the mark survives into helpers that
    # escape their text arguments.
    def self.raw(value)
      SafeString.new(value.to_s)
    end

    # Rails' `content_for?` is `present?`-based: a slot holding only
    # whitespace counts as UNSET. The block form always deposits the
    # template's newlines, so a guard-suppressed `content_for :subnav
    # do` leaves "\n  " in the slot — the shared `empty?` test then
    # renders an empty <header id="subnav"> Rails wouldn't (seen on
    # /u/:username viewing yourself). Shared runtime keeps the plain
    # `empty?` form; blog never deposits whitespace-only slots.
    def self.content_for?(slot)
      !get_slot(slot).strip.empty?
    end
  end
end
