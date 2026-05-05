# Broadcasts — turbo-stream emit bridge.
#
# The model lowerer's `broadcasts_to` expansion produces calls like
# `Broadcasts.prepend(stream: "x", target: "y", html: "...")` from
# inside model callback methods (`after_create`, `after_update`,
# etc.). This shim adapts those calls to the cable broadcaster.
#
# State is held in a module-level Array so tests can assert on what
# was emitted; production also forwards each entry through the
# installed broadcaster (typically a WebSocket fan-out via Cable).

module Broadcasts
  alias BroadcasterFn = Proc(String, String, Nil)

  @@broadcaster : BroadcasterFn? = nil
  @@log = [] of NamedTuple(action: String, stream: String, target: String, html: String)

  # Production server installs a broadcaster that pumps fragments
  # over the cable; tests / CLI runs leave it unset and calls
  # become silent no-ops (the in-memory log is still populated so
  # tests can inspect emit ordering).
  def self.install_broadcaster(fn : BroadcasterFn?) : Nil
    @@broadcaster = fn
  end

  def self.reset_log! : Nil
    @@log.clear
  end

  def self.log
    @@log.dup
  end

  def self.append(*, stream : String, target : String, html : String) : Nil
    record("append", stream, target, html)
  end

  def self.prepend(*, stream : String, target : String, html : String) : Nil
    record("prepend", stream, target, html)
  end

  def self.replace(*, stream : String, target : String, html : String) : Nil
    record("replace", stream, target, html)
  end

  def self.remove(*, stream : String, target : String) : Nil
    record("remove", stream, target, "")
  end

  private def self.record(action : String, stream : String, target : String, html : String) : Nil
    @@log << {action: action, stream: stream, target: target, html: html}
    if fn = @@broadcaster
      fn.call(stream, render_fragment(action, target, html))
    end
    nil
  end

  # Compose the `<turbo-stream>` fragment. Pure; doesn't touch the
  # log — used by tests and transport layers that need to ship the
  # fragment over the wire.
  def self.render_fragment(action : String, target : String, html : String = "") : String
    if action == "remove"
      %(<turbo-stream action="remove" target="#{target}"></turbo-stream>)
    else
      %(<turbo-stream action="#{action}" target="#{target}"><template>#{html}</template></turbo-stream>)
    end
  end
end
