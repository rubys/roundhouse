# Rack adapter for the CRuby target.
#
# Puma loads this file, mounts the returned app, and serves HTTP/1.1.
# Each request goes Rack env → Main.run_rack → Rack tuple, directly: a
# Rack env already carries the CGI-style keys dispatch reads
# (REQUEST_METHOD / PATH_INFO / QUERY_STRING / CONTENT_* / HTTP_COOKIE)
# plus `rack.input`, so there is no env remap, and `run_rack` builds the
# `[status, headers, [body]]` tuple itself rather than serializing a CGI
# byte stream that this file would then re-parse. (The CGI byte form is
# still produced by `Main.run` for the one-shot script path and the
# tests; it is just not on the serving hot path.)

require "rack"
require_relative "main"
require_relative "cable"

Main.configure_default_adapter!

# Register the Cable registry as the broadcasts transport: every
# `Broadcasts.record` call from model callbacks now also fans out
# the rendered `<turbo-stream>` to every WS connection subscribed
# to that stream name.
Broadcasts.set_transport(Cable::Registry)

# Static asset serving — `rake assets` lays files out under
# `static/assets/*` (CSS + JS) plus root-level icons. Rack::Static
# intercepts those URLs before the Rack adapter dispatches to
# Main.run_rack. Anything else falls through to the dynamic Router.
use Rack::Static, urls: ["/assets", "/icon.png", "/icon.svg"], root: "static"

app = lambda do |env|
  # WebSocket upgrade: `/cable`. Hijack the socket from Puma and
  # spawn a per-connection thread that runs the read loop. The Rack
  # tuple returned (-1, {}, []) is Rack's convention for "the
  # response was handled out-of-band" — Puma stops touching the
  # connection.
  if env["PATH_INFO"] == "/cable"
    socket = env["rack.hijack"].call
    conn = Cable::Connection.new(env, socket)
    Thread.new { conn.run }
    return [-1, {}, []]
  end

  # Lease one pooled DB connection for the whole request so concurrent
  # Puma worker threads each read/write through their own handle rather
  # than serializing on a single shared one. `run_rack` reads the Rack
  # env directly and returns the response tuple.
  Db.with_connection { Main.run_rack(env) }
end

run app
