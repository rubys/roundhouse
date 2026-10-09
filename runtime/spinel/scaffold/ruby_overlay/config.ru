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
require_relative "runtime/gzip_cache"

Main.configure_default_adapter!

# Every WebSocket client holds a socket, and all of them live in this
# process. At the soft RLIMIT_NOFILE most environments start with — 1024,
# including Docker's default (soft 1024, hard 524288) — the 1,000-client tier
# stops at ~991 connections: accept() fails with EMFILE and nothing else is
# served. Raise the soft limit to the hard one, as Go's runtime does at
# startup and as the once-campfire Ruby port's server does. A runtime that
# cannot (or a hard limit already reached) keeps what it has.
begin
  soft, hard = Process.getrlimit(:NOFILE)
  Process.setrlimit(:NOFILE, hard, hard) if soft < hard
rescue NotImplementedError, Errno::EPERM, Errno::EINVAL
end

# Serving, so WAL checkpoints move off the request path: each process
# that serves runs them on a background thread instead of inside some
# request's COMMIT (Db.checkpoint_in_background!).
Db.checkpoint_in_background!

# And jobs run off it: `perform_later` queues the work for a drain
# thread in each serving process instead of running it inside the
# request (ActiveJob.drain_in_background!).
ActiveJob.drain_in_background!

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
# Rails' ActionDispatch::Static: a GET/HEAD for a file that exists under
# public/ (robots.txt, the error pages) is served ahead of the routes.
# Not `Rack::Static, urls: [""], cascade: true`: Rack::Files answers a
# POST with 405, which does not cascade, and every form post died there.
PUBLIC_FILES = Rack::Files.new("public")
use(Class.new do
  def initialize(app) = @app = app

  def call(env)
    path = env["PATH_INFO"].to_s
    if %w[GET HEAD].include?(env["REQUEST_METHOD"]) && !path.include?("..") &&
       File.file?(File.join("public", path))
      PUBLIC_FILES.call(env)
    else
      @app.call(env)
    end
  end
end)

# NOTE: no Rack::MethodOverride here — Rack 3 inputs aren't
# rewindable, so that middleware would consume `rack.input` before
# `dispatch_core`'s own body parse. The `_method` override lives in
# `Main.dispatch_core_inner` instead, where it also covers the CGI and
# future spinel serving shapes.

# Campfire's own config.ru is `use Rack::Deflater` then `run` the app.
# GzipCache is that plus a body cache: the same identity HTML is not
# deflated on every wrk GET. HTML only — Static sits outside this
# lambda, so CSS/JS stay identity. NOT around the whole app: the
# hijack tuple `[-1, {}, []]` has no skip, so gzip wraps only run_rack.
gzip = GzipCache.wrap(lambda { |env|
  Db.with_connection { Main.run_rack(env) }
})

app = lambda do |env|
  # WebSocket upgrade: `/cable`. Cable hijacks the socket out of Puma
  # and hands it to the reactor thread, which owns every connection
  # from that point on; this returns as soon as the attach is queued.
  # The Rack tuple (-1, {}, []) is Rack's convention for "the response
  # was handled out-of-band" — Puma stops touching the connection.
  #
  # `upgrade` runs the app's `ApplicationCable::Connection#connect`
  # first and answers nil when the app rejected the handshake, so an
  # unauthenticated client gets a 401 on an intact Rack connection
  # rather than a socket that opens and then goes quiet. It needs a DB
  # handle for the same reason a request does — campfire's `connect`
  # loads a `Session` — so it leases one the same way.
  #
  # The Origin is checked before any of that, as Action Cable orders it
  # (runtime/request_forgery_protection.rb): a handshake from another
  # site is Rails' 404, and the app's `connect` never runs for it.
  if env["PATH_INFO"] == "/cable"
    unless ActionController::RequestForgeryProtection.cable_origin_allowed?(
        env["HTTP_ORIGIN"].to_s,
        ActionController::RequestForgeryProtection.base_url_for(
          env["HTTP_HOST"].to_s, env["HTTPS"].to_s, env["HTTP_X_FORWARDED_PROTO"].to_s),
        Rails.env.development?)
      return [404, { "content-type" => "text/plain" }, ["Page not found"]]
    end
    if Db.with_connection { Cable.upgrade(env) }
      return [-1, {}, []]
    else
      return [401, { "content-type" => "text/plain" }, ["Unauthorized\n"]]
    end
  end

  # Lease one pooled DB connection for the whole request so concurrent
  # Puma worker threads each read/write through their own handle rather
  # than serializing on a single shared one. `run_rack` reads the Rack
  # env directly and returns the response tuple. Gzip sits on this path
  # only — see gzip above.
  gzip.call(env)
end

run app
