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
  TRANSPORTS = []

  def self.reset_log!
    LOG.clear
  end

  def self.log
    LOG.dup
  end

  # The transport responds to `broadcast(stream, fragment_html)` and
  # owns its own thread-safety. Nil-transport (test environment, spinel
  # tests, CGI one-shots) means `record` only appends to LOG.
  def self.set_transport(transport)
    TRANSPORTS.clear
    TRANSPORTS << transport
  end

  def self.append(stream:, target:, html:)
    record(action: :append, stream: stream, target: target, html: html)
  end

  def self.prepend(stream:, target:, html:)
    record(action: :prepend, stream: stream, target: target, html: html)
  end

  def self.replace(stream:, target:, html:)
    record(action: :replace, stream: stream, target: target, html: html)
  end

  def self.remove(stream:, target:)
    record(action: :remove, stream: stream, target: target, html: "")
  end

  def self.record(action:, stream:, target:, html:)
    entry = { action: action, stream: stream, target: target, html: html }
    LOG << entry
    if TRANSPORTS.length > 0
      fragment = render_fragment(action: action, target: target, html: html)
      TRANSPORTS[0].broadcast(stream, fragment)
    end
    nil
  end

  # Compose the actual <turbo-stream> markup. Pure: doesn't touch the
  # log — used by tests and (eventually) by transport layers that
  # need to ship the fragment over the wire.
  def self.render_fragment(action:, target:, html: "")
    if action == :remove
      %(<turbo-stream action="remove" target="#{target}"></turbo-stream>)
    else
      %(<turbo-stream action="#{action}" target="#{target}"><template>#{html}</template></turbo-stream>)
    end
  end
end
