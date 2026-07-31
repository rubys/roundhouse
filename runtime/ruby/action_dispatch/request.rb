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

    def referrer
      @referer
    end
  end
end
