# Top-level entry point for the spinel-blog application.
#
# Reads a CGI request from ENV + $stdin, dispatches through the router
# + controller, writes the CGI response to $stdout. The shape spinel
# can ingest (no sockets, just env-vars + stdin + stdout) and the
# shape any CGI-aware web server can drive.
#
# Library usage (from tests):
#   require_relative "main"
#   Main.run(env_hash, body_io, response_io)
#
# Script usage:
#   REQUEST_METHOD=GET PATH_INFO=/articles ruby main.rb
# Or behind a CGI-aware server:
#   AddHandler cgi-script .rb       (apache)
#   alias /blog /path/to/main.rb    (nginx + fcgiwrap)

$LOAD_PATH.unshift(File.expand_path("runtime", __dir__))
$LOAD_PATH.unshift(File.expand_path("app",     __dir__))
$LOAD_PATH.unshift(File.expand_path("config",  __dir__))

require "sqlite_adapter"
require "active_record"
require "schema"
require "action_dispatch"
require "action_controller"
require "broadcasts"
require "cgi_io"
require "routes"
require "views"

module Main
  module_function

  # Dispatch one request. Pure function over (env, stdin, stdout) — no
  # global I/O. Tests construct env hashes + StringIO and call this
  # directly; the script path at the bottom of this file calls it
  # with real ENV + $stdin + $stdout.
  def run(env, stdin, stdout)
    ViewHelpers.reset_slots!
    Broadcasts.reset_log!

    request = CgiIo.parse_request(env, stdin)
    matched = Router.match(request[:method], request[:path], Routes::TABLE)
    if matched.nil?
      CgiIo.write_response(stdout, 404, "<h1>404 Not Found</h1>")
      return
    end

    controller = matched[:controller].new
    merged = matched[:path_params].dup
    request[:params].each { |k, v| merged[k] = v }
    controller.params  = ActionController::Parameters.new(merged)
    controller.session = {}

    # Decode inbound flash from cookies. Each flash key carries via
    # its own cookie (`flash_notice`, `flash_alert`) so the cookie
    # plumbing stays format-free.
    cookies = request[:cookies] || {}
    inbound_flash = {}
    inbound_flash[:notice] = cookies[:flash_notice] if cookies.key?(:flash_notice)
    inbound_flash[:alert]  = cookies[:flash_alert]  if cookies.key?(:flash_alert)
    controller.flash = inbound_flash

    controller.request_method = request[:method]
    controller.request_path   = request[:path]

    begin
      controller.process_action(matched[:action])
    rescue ActiveRecord::RecordNotFound
      CgiIo.write_response(stdout, 404, "<h1>404 Not Found</h1>")
      return
    end

    # Encode outbound flash. On render: clear the inbound cookies
    # (the action used flash for display; the next request shouldn't
    # see the same notice again). On redirect: ship the controller's
    # current flash as cookies for the next request to consume.
    out_cookies = {}
    if controller.location.nil?
      out_cookies[:flash_notice] = nil if cookies.key?(:flash_notice)
      out_cookies[:flash_alert]  = nil if cookies.key?(:flash_alert)
      page = Views::Layouts.application(controller.body)
      CgiIo.write_response(stdout, controller.status, page, set_cookies: out_cookies)
    else
      out_cookies[:flash_notice] = controller.flash[:notice] unless controller.flash[:notice].nil?
      out_cookies[:flash_alert]  = controller.flash[:alert]  unless controller.flash[:alert].nil?
      # Redirects: short-circuit body to a one-line "redirecting"
      # message; real browsers follow the Location header without
      # rendering the body anyway.
      CgiIo.write_response(stdout, controller.status,
        %(<a href="#{controller.location}">Redirecting</a>),
        location: controller.location,
        set_cookies: out_cookies)
    end
  end

  # First-time setup. Idempotent: skips when already configured (so
  # tests that load main.rb don't conflict with their own test_helper
  # setup). When BLOG_DB env var points at a file path, persists
  # state across invocations; otherwise an `:memory:` DB is fresh per
  # process (fine for one-shot smoke tests).
  def configure_default_adapter!
    return unless ActiveRecord.adapter.nil?
    SqliteAdapter.configure(ENV["BLOG_DB"] || ":memory:")
    ActiveRecord.adapter = SqliteAdapter
    Schema.load!(SqliteAdapter)
  end
end

# Auto-run only when invoked as a script (`ruby main.rb`). When loaded
# via `require_relative "main"` from tests, the dispatch isn't
# triggered — tests call Main.run themselves with constructed I/O.
if __FILE__ == $PROGRAM_NAME
  Main.configure_default_adapter!
  Main.run(ENV.to_h, $stdin, $stdout)
end
