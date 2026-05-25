# Per-request store for `content_for` / `yield` slot values.
#
# Each HTTP request gets its own `Slots` instance threaded through the
# controller→view call chain. Replaces the prior module-level `@slots`
# on `ActionView::ViewHelpers`, which was shared across all concurrent
# requests in a process — safe under MRI's GVL (single-threaded
# request handling) but a data race under Go's parallel goroutines and
# a semantic race under Puma's multi-thread workers (thread A's
# `content_for(:title, ...)` could be overwritten by thread B between
# A's set and A's yield(:title)).
#
# The Slots instance is built per request at dispatch entry and passed
# into every view function as an explicit positional arg. Views call
# `slots.set(:title, "...")` (was `ViewHelpers.content_for_set(...)`),
# `slots.get(:title)` (was `ViewHelpers.content_for_get(...)`), and
# `slots[:title]` style via `bracket_get` (was `ViewHelpers.get_slot`).
module ActionView
  class Slots
    def initialize
      @slots = {}
    end

    def reset!
      @slots = {}
      nil
    end

    # `content_for(:title, "X")` setter form. Stores the value;
    # returns nil so statement-form call sites in lowered view bodies
    # don't accidentally feed into `io << ...`.
    def set(slot, value)
      @slots[slot] = value
      nil
    end

    # `content_for(:title)` getter form — returns nil for unset slots
    # so callers can write `slots.get(:title) || "default"`. Mirrors
    # Hash#fetch(slot, nil) — Crystal's strict Hash#[] raises KeyError,
    # `fetch(k, nil)` produces nil-on-missing on both targets.
    def get(slot)
      @slots.fetch(slot, nil)
    end

    # `yield :title` lookup — returns "" for unset slots so the layout
    # can splice the result directly into HTML without a nil check.
    def bracket_get(slot)
      @slots[slot] || ""
    end

    # Body-yield slot. Layouts call `slots.get_yield` to splice the
    # rendered action body in; controllers' render path calls
    # `slots.set_yield(rendered_body)` before invoking the layout.
    def get_yield
      @slots[:__body__] || ""
    end

    def set_yield(content)
      @slots[:__body__] = content
      nil
    end
  end
end
