# Broadcasts — in-memory log of Turbo Stream fragments produced by
# model after_*_commit hooks. The log is the test-visible contract;
# transport (WebSocket fan-out) is the live-server contract,
# registered by the target overlay (CRuby's `config.ru` hands in a
# Cable registry; spinel will pass an sphttp-side equivalent).
#
# State is held in module-level constant Arrays. Spinel supports
# constants and array mutation; module-level instance variables are
# more uncertain, so we deliberately use the constant form. Same
# pattern for the (at most one) transport hook — single-element
# Array as a settable holder.
module Broadcasts
  LOG = []

  # Type-seed stub: pins TRANSPORTS' element type so spinel can
  # dispatch `broadcast(stream, fragment)` correctly inside `record`.
  # The real transport is wired by the target overlay (CRuby's
  # config.ru calls `set_transport(Cable::Registry)` at boot, which
  # clears+replaces this stub; spinel-AOT will pass an sphttp-side
  # equivalent once the substrate lands). Without this seed, spinel
  # has no caller of `set_transport` and defaults its `transport`
  # param to int, poisoning TRANSPORTS' element type.
  class SeedTransport
    def broadcast(stream, fragment)
      nil
    end
  end

  # Seeded with an INSTANCE, not left empty: an always-empty literal
  # gives spinel nothing to type the array from (`set_transport`'s
  # param is caller-typed, and its only caller is CRuby's config.ru —
  # outside the spinel compile graph), so every TRANSPORTS operation
  # (`length`/`[]`/`clear`/`<<` and the `broadcast` dispatch) sat
  # behind unresolved-call gate arms. The old gate silently no-op'd
  # them; spinel 1356cb14's strict gate raises at first save
  # (after_commit hook → record → TRANSPORTS.length). The stub
  # broadcast is a nil no-op, so the seeded holder behaves identically
  # to the empty one until a real transport replaces it.
  TRANSPORTS = [SeedTransport.new]

  def self.reset_log!
    LOG.clear
  end

  def self.log
    LOG.dup
  end

  # THE ONE WRITE PATH onto the log. `record` and both
  # `ActionCable::Server#broadcast`s (the spinel runtime's and the CRuby
  # overlay's) append through here, never onto `LOG` directly, because
  # `runtime/spinel/thread_state.rb` reopens `log_append`, `log` and
  # `reset_log!` to keep one log per thread -- a writer on the constant
  # would be invisible to every reader the moment that file loads. It
  # was: the overlay's raw broadcast went onto `LOG` while the readers
  # asked the per-thread log, and campfire's two unread-room assertions
  # answered 0 (CI 256/42 -> 254/41, 2026-09-02).
  def self.log_append(entry)
    LOG << entry
    nil
  end

  # The transport responds to `broadcast(stream, fragment_html)` and
  # owns its own thread-safety. Nil-transport (test environment, spinel
  # tests, CGI one-shots) means `record` only appends to LOG.
  def self.set_transport(transport)
    TRANSPORTS.clear
    TRANSPORTS << transport
  end

  # The four actions go out through `Turbo::StreamsChannel` (below)
  # rather than straight to `record`. See the note on that module: it is
  # the seam Rails apps MOCK, and one hop is what makes their own tests
  # runnable against this runtime.

  # `attributes:` is the EXTRA attribute text the turbo-stream element
  # carries — ` maintain_scroll="true"` — already rendered by
  # `lower::controller_to_library::broadcasts`, which is where the
  # literal hash the app wrote can be read. A String rather than a hash
  # so every target's twin holds it without its own markup renderer;
  # empty is the overwhelming case and writes exactly the element these
  # methods have always written.

  def self.append(stream:, target:, html:, attributes: "")
    Turbo::StreamsChannel.broadcast_append_to(stream, target: target, html: html, attributes: attributes)
  end

  def self.prepend(stream:, target:, html:, attributes: "")
    Turbo::StreamsChannel.broadcast_prepend_to(stream, target: target, html: html, attributes: attributes)
  end

  def self.replace(stream:, target:, html:, attributes: "")
    Turbo::StreamsChannel.broadcast_replace_to(stream, target: target, html: html, attributes: attributes)
  end

  def self.remove(stream:, target:, attributes: "")
    Turbo::StreamsChannel.broadcast_remove_to(stream, target: target, attributes: attributes)
  end

  def self.record(action:, stream:, target:, html:, attributes: "")
    # `attributes` rides along so a reader can rebuild the EXACT
    # fragment this dispatched. `ActionCable::Server#pubsub` is that
    # reader — an app's own test asks the pubsub queue what was
    # published, and a fragment rebuilt without its attributes is not
    # what went over the wire.
    entry = { action: action, stream: stream, target: target, html: html, attributes: attributes }
    log_append(entry)
    # Unconditional dispatch — TRANSPORTS always holds exactly one
    # transport (the no-op SeedTransport until an overlay replaces it),
    # so there is no empty case to guard. Null-object shape: the seed
    # absorbs test/CGI-one-shot broadcasts at the cost of composing the
    # fragment string nobody ships.
    fragment = render_fragment(action: action, target: target, html: html, attributes: attributes)
    TRANSPORTS[0].broadcast(stream, fragment)
    nil
  end

  # Compose the actual <turbo-stream> markup. Pure: doesn't touch the
  # log — used by tests and (eventually) by transport layers that
  # need to ship the fragment over the wire.
  def self.render_fragment(action:, target:, html: "", attributes: "")
    turbo_stream_fragment(action.to_s, target, html, attributes)
  end

  # Compose the `<turbo-stream>` element for a `turbo_stream.<action>`
  # call in a `.turbo_stream.erb` template. Positional and String-typed
  # on purpose: it is the ONE shape the view lowerer emits on every
  # target. `render_fragment` above (keyword args, Symbol action) is the
  # model-broadcast spelling and now delegates here, so the markup has a
  # single owner per target.
  #
  # `attributes` is rendered attribute text carrying its own leading
  # space, written BEFORE `action`/`target` because that is where
  # turbo-rails' `tag.turbo_stream(template, **attributes, action:,
  # target:)` puts it. Optional, so the three-argument call every view
  # lowerer emits is unchanged.
  def self.turbo_stream_fragment(action, target, html, attributes = "")
    if action == "remove"
      %(<turbo-stream#{attributes} action="remove" target="#{target}"></turbo-stream>)
    else
      %(<turbo-stream#{attributes} action="#{action}" target="#{target}"><template>#{html}</template></turbo-stream>)
    end
  end

  # Module-load param-type pin: a direct `broadcast(String, String)`
  # call so spinel types SeedTransport#broadcast's params (it doesn't
  # propagate them back from the `TRANSPORTS[0].broadcast(stream,
  # fragment)` dispatch in `record`). The holder itself is seeded at
  # the constant (see TRANSPORTS above); overlays that wire a real
  # transport replace it via `set_transport`.
  TRANSPORTS[0].broadcast("", "")
end

# `Turbo::StreamsChannel` — the seam a Rails app's own tests MOCK, and
# the four `broadcast_*_to` methods that reach `record` above — USED TO
# LIVE HERE. It is one class up in `turbo_streams.rb` now, beside the
# channel half of the same constant.
#
# WHY IT MOVED. turbo-rails has ONE `Turbo::StreamsChannel`: an
# `ActionCable::Channel::Base` subclass that also carries the class-level
# broadcast API. Splitting it left this file defining a MODULE by that
# name and `turbo_streams.rb` trying to open a CLASS by it, which is a
# `TypeError` at require time — the whole campfire suite went to 0 of 288
# on it. The definitions had to join, and they joined in the file named
# after the constant rather than in this one, which is named after
# `Broadcasts`.
#
# This file keeps `Broadcasts.record`, which is what those methods call
# and what the log belongs to; `turbo_streams.rb` requires it.
