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
  LOG = []

  def self.reset_log!
    LOG.clear
  end

  def self.log
    LOG.dup
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
    write_to_disk(entry) unless ENV["BROADCAST_DIR"].to_s.empty?
    nil
  end

  # File-based IPC for a future native HTTP front end (civetweb,
  # tracked under project_ruby_dev_server_retirement memory). When
  # ENV["BROADCAST_DIR"] is set, every broadcast is also written as
  # a `.frag` file in that directory; a watcher process consumes
  # them and fans out the contents over WebSockets to subscribed
  # Turbo clients. The previous CRuby dev server that owned this
  # contract was retired in the rubocop_spinel cleanup.
  #
  # Filename: <safe_stream>__<microsecond_ts>.frag
  #   - stream name encodes which subscribers receive this fragment
  #     (the dev server splits filename on "__" to recover it)
  #   - microsecond timestamp gives temporal ordering + uniqueness
  #     (single broadcast per microsecond per stream is the floor)
  # Content: the rendered <turbo-stream> HTML, as produced by
  # render_fragment — no JSON envelope (the dev server constructs
  # the ActionCable wire-format envelope when forwarding to WS).
  def self.write_to_disk(entry)
    dir = ENV["BROADCAST_DIR"]
    return if dir.nil? || dir.empty?
    fragment = render_fragment(action: entry[:action], target: entry[:target], html: entry[:html])
    safe = entry[:stream].gsub(/[^a-zA-Z0-9_-]/, "_")
    ts   = Time.now.utc.strftime("%Y%m%dT%H%M%S%6N")
    final = File.join(dir, "#{safe}__#{ts}.frag")
    # Atomic publish: write to a `.tmp` sibling first, then rename.
    # The watcher globs `*.frag` only, so it never observes a
    # half-written file. Rename is atomic on POSIX filesystems.
    tmp = "#{final}.tmp"
    File.write(tmp, fragment)
    File.rename(tmp, final)
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
