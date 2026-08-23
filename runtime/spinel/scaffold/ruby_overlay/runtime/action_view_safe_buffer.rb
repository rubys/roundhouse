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
      # CGI.escapeHTML is the C-accelerated escape — same five entities,
      # byte-identical output incl. &#39; for apostrophe (verified
      # against HTML_ESCAPES, and against the 26-route parity dumps).
      # Rails' own ERB::Util.html_escape rides this same C path; the
      # pure-Ruby gsub form profiled as a visible String#gsub band,
      # though at today's ~137ms/iter baseline the delta is inside
      # run-to-run noise — this is the strictly-better form, not a
      # measured win.
      CGI.escapeHTML(s)
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

# `ActionText::Content#to_s` is MARKUP, and Rails says so.
#
# Real Action Text renders `to_s` through the `action_text/contents
# /_content` partial, so it comes back as a SafeBuffer; the shared
# runtime returns the bare `@html` String because it has no safe-string
# type to return. That difference is invisible until something escapes
# the value — and campfire's `message_presentation` does exactly that,
# `h(ContentFilters::TextMessagePresentationFilters.apply(message.body
# .body))`. Without the mark the body renders as `&lt;b&gt;3.4.10&lt;
# /b&gt;`: the tags of every formatted message, visible as text.
#
# Here rather than in `runtime/ruby/action_text.rb` because `SafeString`
# is a CRuby-overlay type. A strict target gets its safe-string story
# with the rest of them.
module ActionText
  class Content
    def to_s
      SafeString.new(rendered_html)
    end

    # Rails renders `to_s` THROUGH `layouts/action_text/contents/
    # _content`, which is why a rich text arrives wrapped. campfire
    # ships that layout (`<div class="trix-content">`), the emit
    # produces it as `Views::Layouts::ActionText::Contents.content` —
    # and nothing called it, so every message body rendered one div
    # short of Rails. The shared runtime's comment asserted the wrapper
    # was "view-side decoration that the emitted views apply
    # themselves"; measured, no emitted view applies it.
    #
    # Guarded on the constant because the layout is per-APP: an app that
    # ships no `_content` template has no wrapper to apply, which is
    # also Rails' behaviour (Action Text falls back to rendering the
    # fragment bare).
    def rendered_html
      if defined?(::Views::Layouts::ActionText::Contents)
        ::Views::Layouts::ActionText::Contents.content(@html)
      else
        @html
      end
    end

    # …and the other half of the same decision, which is why it sits
    # here rather than anywhere else.
    #
    # Rails splits a value object's two answers: `to_s` RENDERS, and the
    # JSON form is the VALUE. campfire's own webhook test pins the
    # split — the payload it asserts for `message.body.body` is
    # `"First post!"`, unwrapped, on the same content the page renders
    # inside a `<div class="trix-content">`.
    #
    # Without this, `{ body: content }.to_json` reaches the stdlib
    # generator, which has no `to_json` for this class and falls back to
    # `to_s` — so the moment `to_s` started rendering, every webhook
    # payload shipped the wrapper and `WebhookTest#test_payload` stopped
    # matching its own stub. The shared runtime's `as_json` already
    # answers `@html`; this makes the stdlib encoder ask.
    def to_json(*args)
      as_json.to_json(*args)
    end
  end
end
