# Broadcasts — in-memory log of Turbo Stream fragments produced by
# model after_*_commit hooks. The destination is a test-shaped log;
# in production it would be a WebSocket fan-out (Action Cable) or a
# BroadcastChannel postMessage (juntos in-browser SPA pattern). The
# log is the contract — once fragments land in the log, transport is
# orthogonal.
#
# State is held in a module-level constant Array. Spinel supports
# constants and array mutation; module-level instance variables are
# more uncertain, so we deliberately use the constant form.
module Broadcasts
  module_function

  LOG = []

  def reset_log!
    LOG.clear
  end

  def log
    LOG.dup
  end

  def append(stream:, target:, html:)
    record(action: :append, stream: stream, target: target, html: html)
  end

  def prepend(stream:, target:, html:)
    record(action: :prepend, stream: stream, target: target, html: html)
  end

  def replace(stream:, target:, html:)
    record(action: :replace, stream: stream, target: target, html: html)
  end

  def remove(stream:, target:)
    record(action: :remove, stream: stream, target: target, html: "")
  end

  def record(action:, stream:, target:, html:)
    LOG << { action: action, stream: stream, target: target, html: html }
    nil
  end

  # Compose the actual <turbo-stream> markup. Pure: doesn't touch the
  # log — used by tests and (eventually) by transport layers that
  # need to ship the fragment over the wire.
  def render_fragment(action:, target:, html: "")
    if action == :remove
      %(<turbo-stream action="remove" target="#{target}"></turbo-stream>)
    else
      %(<turbo-stream action="#{action}" target="#{target}"><template>#{html}</template></turbo-stream>)
    end
  end
end
