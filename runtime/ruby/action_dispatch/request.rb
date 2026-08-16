# ActionDispatch::Request — the request-object surface controllers,
# filters, and helpers reach (`request.remote_ip`, `request.referer`,
# `request.xhr?`, `request.env[...]`, `request.get?`). Typed fields the
# dispatcher assigns from its transport (Tep under the spinel binary),
# not an env-hash bag — per-field types keep every read concrete under
# AOT. `env` remains as the one compat bag: lobsters reads
# `env["HTTP_USER_AGENT"]` and writes scratch keys
# (`exception_notifier.exception_data`), shapes the typed fields can't
# carry.
#
# Loaded explicitly by the spinel scaffold's main.rb (not from the
# action_dispatch require chain): the CRuby tree keeps its overlay
# Request (CGI-env-backed, runtime/action_dispatch_request.rb) and must
# not blend the two shapes.
module ActionDispatch
  class Request
    attr_accessor :remote_ip
    attr_accessor :path
    attr_accessor :query_string
    # Rack's SCRIPT_NAME — the prefix the app is mounted under, "" at
    # the root. Campfire's cable helper joins it with Action Cable's
    # mount path to build the socket URL, so every page that renders
    # the layout reads it.
    attr_accessor :script_name
    attr_accessor :request_method
    attr_accessor :referer
    attr_accessor :host
    attr_reader :format
    attr_accessor :body
    attr_accessor :env

    def initialize
      @remote_ip = "127.0.0.1"
      @path = "/"
      @query_string = +""
      @script_name = +""
      @request_method = "GET"
      @referer = +""
      @host = "localhost"
      @format = "html"
      @body = +""
      @env = {}
    end

    # Rails accepts a symbol (`request.format = :json`); store the
    # canonical string.
    def format=(value)
      @format = value.to_s
    end

    def get?
      @request_method == "GET"
    end

    def post?
      @request_method == "POST"
    end

    def xhr?
      @env.fetch("HTTP_X_REQUESTED_WITH", "").to_s == "XMLHttpRequest"
    end

    def fullpath
      if @query_string == ""
        @path
      else
        @path + "?" + @query_string
      end
    end

    # No middleware rewrites paths here, so original_* == current.
    def original_fullpath
      fullpath
    end

    # Scheme + host, no path — what Rails builds absolute URLs from.
    # The scheme is a read of the one env key that carries it; a
    # transport that terminates TLS elsewhere (every lane here) reports
    # http, which is what the CRuby tree's overlay Request reports too.
    def base_url
      if @env.fetch("HTTPS", "").to_s == "on"
        "https://" + @host
      else
        "http://" + @host
      end
    end

    # Absolute URL of this request. Feed templates interpolate it as
    # the channel link (lobsters' home/rss.rbuilder), which is why the
    # spinel tree needs it and not just the CRuby overlay's twin.
    def original_url
      base_url + fullpath
    end

    # Rails' `request.url` is `original_url` — same string, and the name
    # app code reaches for (campfire's `request_authentication` stores it
    # as the post-login return path). Kept as its own method rather than
    # an alias so the strict targets see a real definition.
    def url
      original_url
    end

    def referrer
      @referer
    end

    # The User-Agent header. campfire's auth spine records it on every
    # Session row (`start_new_session_for`) and `deny_bots` filters on
    # it, so every sign-in reads it. Was a hole we opened ourselves:
    # the walk's stub ledger carried it as "ours to implement".
    def user_agent
      @user_agent
    end

    def user_agent=(value)
      @user_agent = value
    end

    # Retained so `Request.for`'s signature matches the overlay twin's.
    # No `[]` delegator here: the overlay has one because lobsters
    # writes `request[:format]`, and adding an untyped-returning reader
    # nothing calls just spends runtime-typing budget.
    def params
      @params
    end

    def params=(value)
      @params = value
    end

    # Build a request from a CGI/Rack-shaped env hash.
    #
    # THE SHARED CONSTRUCTOR. This class and the CRuby overlay's twin
    # (`runtime/action_dispatch_request.rb`) hold their state
    # differently — this one in attributes, that one derived from a
    # retained `@env` — so they cannot share a `new`. They can share
    # this, which is what lets one caller build a request on either
    # target. The test harness is that caller; before it existed,
    # `controller.request` was simply nil in every controller test and
    # campfire's first filter died on `request.remote_ip`.
    def self.for(env, params = {})
      r = new
      r.env = env
      r.params = params
      r.request_method = env["REQUEST_METHOD"] || "GET"
      r.path = env["PATH_INFO"] || "/"
      r.query_string = env["QUERY_STRING"] || ""
      r.script_name = env["SCRIPT_NAME"] || ""
      r.host = env["HTTP_HOST"] || env["SERVER_NAME"] || "localhost"
      r.remote_ip = env["REMOTE_ADDR"] || "127.0.0.1"
      r.referer = env["HTTP_REFERER"] || ""
      r.user_agent = env["HTTP_USER_AGENT"] || ""
      r
    end
  end
end
