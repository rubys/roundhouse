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
module ActionDispatch
  class Request
    attr_reader :env
    attr_accessor :params

    def initialize(env, params = {})
      @env = env
      @params = params
    end

    def [](key)
      @params[key.to_s]
    end

    def request_method
      (@env["REQUEST_METHOD"] || "GET").upcase
    end

    def get? = request_method == "GET"
    def post? = request_method == "POST"

    def path
      @env["PATH_INFO"] || "/"
    end

    def query_string
      @env["QUERY_STRING"] || ""
    end

    def fullpath
      query_string.empty? ? path : "#{path}?#{query_string}"
    end

    # No middleware rewrites paths here, so original_* == current.
    def original_fullpath = fullpath

    def original_url = "#{base_url}#{fullpath}"

    def base_url
      scheme = @env["HTTPS"] == "on" ? "https" : "http"
      host = @env["HTTP_HOST"] || @env["SERVER_NAME"] || "localhost"
      "#{scheme}://#{host}"
    end

    def remote_ip
      @env["REMOTE_ADDR"] || "127.0.0.1"
    end

    def referer
      @env["HTTP_REFERER"]
    end
    alias referrer referer

    def xhr?
      @env["HTTP_X_REQUESTED_WITH"] == "XMLHttpRequest"
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
