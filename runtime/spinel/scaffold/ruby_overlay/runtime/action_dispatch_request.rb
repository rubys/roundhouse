# CRuby-only ActionDispatch::Request over the CGI env.
#
# The request-object surface controllers and filters reach
# (`request.remote_ip`, `request.referer`, `request.xhr?`,
# `request.env[...]=`, `request[:format]`). Built straight from the
# CGI/1.1 env hash the dispatcher already receives — no Rack. Lives on
# the CRuby overlay next to CookieJar: 8 targets don't exercise a
# request object yet, and per the CRuby-first strategy each target
# grows its own when its lobsters turn comes.
#
# `[]` delegates to the request params (Rails: `request[:format]` ==
# `params[:format]`), so the dispatcher hands the merged params in
# alongside the env. `env` is a plain mutable Hash copy — callers
# write scratch keys into it (`exception_notifier.exception_data`),
# which the real ENV object would reject for non-String values.
require "stringio"
require_relative "action_dispatch/headers"

module ActionDispatch
  # `ActionDispatch::TestRequest.create(env)` — see the twin in
  # `runtime/ruby/action_dispatch/request.rb` for what it is for. This
  # lane's Request IS env-backed, so `create` is the constructor and
  # every key the env carries is readable, not just the ones a mapping
  # thought to name.
  module TestRequest
    def self.create(env)
      Request.new(env.each_with_object({}) { |(k, v), h| h[k.to_s] = v })
    end
  end

  class Request
    attr_reader :env
    attr_accessor :params
    # The body's params alone - see the twin in
    # runtime/ruby/action_dispatch/request.rb.
    attr_accessor :request_parameters

    def initialize(env, params = {})
      @env = env
      @params = params
      @request_parameters = {}
      @session_options = {}
    end

    # Rack's per-request session options — see the twin in
    # runtime/ruby/action_dispatch/request.rb. `:skip` is the one key
    # read: the dispatcher writes no session cookie when it is set.
    attr_reader :session_options

    def session_skip?
      @session_options[:skip] == true
    end

    # `request.headers["Accept-Encoding"]` — the shared
    # `ActionDispatch::Http::Headers` over this request's env.
    def headers
      ActionDispatch::Http::Headers.new(env)
    end

    def [](key)
      @params[key.to_s]
    end

    def request_method
      (@env["REQUEST_METHOD"] || "GET").upcase
    end

    def get? = request_method == "GET"
    def post? = request_method == "POST"
    # Rails' "is this a safe request" idiom is `request.get? ||
    # request.head?`, and campfire writes exactly that in
    # `BlockBannedRequests#safe_request?`. This overlay is the Request
    # the ruby-family SERVER actually loads — the `runtime/ruby` copy
    # beside it answers the same questions for the analyzer and the
    # strict targets, so a predicate has to land in both or the one
    # that runs is the one still missing it.
    def head? = request_method == "HEAD"

    def path
      @env["PATH_INFO"] || "/"
    end

    def query_string
      @env["QUERY_STRING"] || ""
    end

    # Rack's SCRIPT_NAME — the prefix the app is mounted under, "" at
    # the root. Campfire's cable helper joins it with Action Cable's
    # mount path to build the socket URL, so every page that renders
    # the layout reads it.
    def script_name
      @env["SCRIPT_NAME"] || ""
    end

    def fullpath
      query_string.empty? ? path : "#{path}?#{query_string}"
    end

    # No middleware rewrites paths here, so original_* == current.
    def original_fullpath = fullpath

    def original_url = "#{base_url}#{fullpath}"
    # Rails' own name for the same string; app code reaches for `url`
    # (campfire stores it as the post-login return path).
    def url = original_url

    # Host (with port when the client sent one), as Rails reports it.
    # The shared `ActionDispatch::Request` has always carried this; the
    # overlay twin did not, and `Rails.application.domain` — the
    # framework default every `_url` helper grounds against — reads it.
    # So an app with no `domain` of its own raised NoMethodError on its
    # first absolute URL: campfire's message row, which links each
    # message by `room_at_message_url`.
    def host
      @env["HTTP_HOST"] || @env["SERVER_NAME"] || "localhost"
    end

    # See the shared twin: HTTPS=on, or a TLS-terminating proxy's
    # X-Forwarded-Proto, which Rack honors unconfigured.
    def ssl?
      return true if @env["HTTPS"] == "on"
      forwarded = @env["HTTP_X_FORWARDED_PROTO"].to_s.split(",").first.to_s.strip.downcase
      return forwarded == "https" unless forwarded.empty?
      @env["rack.url_scheme"].to_s.downcase == "https"
    end

    def protocol
      ssl? ? "https://" : "http://"
    end

    # Twin of the shared class's: the port from the Host header, nil at
    # the scheme's standard one (campfire's default_url_options).
    def optional_port
      h = host
      sep = h.rindex(":")
      return nil if sep.nil? || h.end_with?("]")
      port = h[(sep + 1)..].to_i
      return nil if port == 0 || port == (ssl? ? 443 : 80)
      port
    end

    # Without the scheme's standard port, as the shared twin's: Rails'
    # `host_with_port` writes no `:443`/`:80`, and the CSRF check
    # compares this with an Origin a browser serializes the same way.
    def base_url
      h = host
      default = ssl? ? ":443" : ":80"
      h = h[0, h.length - default.length] if h.end_with?(default)
      "#{protocol}#{h}"
    end

    def remote_ip
      @env["REMOTE_ADDR"] || "127.0.0.1"
    end

    def referer
      @env["HTTP_REFERER"]
    end
    alias referrer referer

    # The User-Agent header. campfire's auth spine records it on every
    # Session row and `deny_bots` filters on it — a hole we opened
    # ourselves (the walk's stub ledger carried it as "ours to
    # implement"). Twin of the shared class's, which stores it.
    def user_agent
      @env["HTTP_USER_AGENT"] || ""
    end

    # Shared constructor — see the twin in
    # `runtime/ruby/action_dispatch/request.rb`. The two classes hold
    # their state differently and so cannot share a `new`; this is the
    # seam one caller uses to build a request on either target.
    def self.for(env, params = {})
      new(env, params)
    end

    # `request.body` — the raw body, as an IO, because that is what
    # Rails hands back and what app code does with it: campfire's bot
    # endpoint reads `reading(request.body) { |b| … }`, which calls
    # `rewind` / `read` / `force_encoding`. A String would answer none
    # of those.
    #
    # The WRITER takes a String (the CGI dispatcher's body, or the test
    # harness's `post url, params: "raw text"`) and wraps it here, so
    # the caller stays target-neutral — the harness is transpiled for
    # spinel too, where `StringIO` is a CRuby thing that does not
    # exist. Wrapping at the overlay boundary keeps that knowledge on
    # the CRuby side, where the rest of this file already lives.
    def body=(value)
      @body = value.is_a?(String) ? StringIO.new(value) : value
    end

    def body
      @body ||= StringIO.new(@env["RAW_POST_DATA"].to_s)
    end

    def xhr?
      @env["HTTP_X_REQUESTED_WITH"] == "XMLHttpRequest"
    end

    def inertia?
      !@env["HTTP_X_INERTIA"].to_s.empty?
    end

    # Query-string params only (Rails' GET-vs-POST split); lobsters'
    # search/time-series pages rebuild URLs from these.
    def query_parameters
      out = {}
      CgiIo.parse_form_into(query_string, out) unless query_string.empty?
      out
    end
  end
end

# `request` accessor on the controller — same overlay-reopen shape as
# `cookies` (runtime/action_controller_cookies.rb).
module ActionController
  class Base
    attr_accessor :request
  end

  # Per-request context reachable from module-function helpers. Rails
  # helpers run in the view context, which delegates `request` to the
  # controller; the emitted helpers are module functions with no such
  # context, so the dispatcher parks the request here (the
  # ActiveSupport::CurrentAttributes pattern) and the Ruby emit path
  # rewrites bare `request` reads in helper/view module bodies to
  # `ActionController::Current.request`. Single-threaded CGI dispatch —
  # plain module state, reset by assignment each request.
  module Current
    class << self
      attr_accessor :request
      # The dispatching controller, parked so module-function helpers
      # can reach per-request session state. Held as the controller
      # (not the session object) because `reset_session` swaps the
      # controller's @session for a fresh instance mid-action — a
      # parked session reference would go stale, and a CSRF token
      # generated during the post-logout render would land in the
      # discarded session instead of the one the dispatch persists.
      attr_accessor :controller
    end

    # The current request's session, or nil outside a dispatch (unit
    # tests construct view helpers without a controller; they get the
    # shared runtime's empty-token behavior).
    def self.session
      c = Current.controller
      c.nil? ? nil : c.session
    end
  end
end
