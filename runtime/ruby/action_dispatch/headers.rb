module ActionDispatch
  module Http
    # `request.headers` — Rails' `ActionDispatch::Http::Headers`: reads a
    # request header by its HTTP name out of the env the request carries.
    # campfire's `CachedResponses` negotiates on `Accept-Encoding` and
    # skips page reuse on `If-None-Match` / `If-Modified-Since` /
    # `Turbo-Frame`, all through it.
    #
    # The name mapping is Rails' `env_name`: a key made only of letters,
    # digits and dashes is an HTTP name — upcased, dashes to underscores,
    # and `HTTP_` in front unless it is one of the CGI variables
    # (`Content-Type` is `CONTENT_TYPE`). Anything else (`HTTP_ACCEPT`,
    # `rack.input`) is already an env key and is read as given.
    #
    # Shared by both Request twins: `runtime/ruby/action_dispatch/
    # request.rb` and the CRuby overlay's `runtime/action_dispatch_request.rb`.
    class Headers
      CGI_VARIABLES = %w[
        AUTH_TYPE CONTENT_LENGTH CONTENT_TYPE GATEWAY_INTERFACE HTTPS
        PATH_INFO PATH_TRANSLATED QUERY_STRING REMOTE_ADDR REMOTE_HOST
        REMOTE_IDENT REMOTE_USER REQUEST_METHOD SCRIPT_NAME SERVER_NAME
        SERVER_PORT SERVER_PROTOCOL SERVER_SOFTWARE
      ].freeze
      HTTP_HEADER = /\A[A-Za-z0-9-]+\z/.freeze

      def initialize(env)
        @env = env
      end

      def [](key)
        @env[Headers.env_name(key)]
      end

      def key?(key)
        @env.key?(Headers.env_name(key))
      end

      def include?(key)
        key?(key)
      end

      def fetch(key, *defaults)
        name = Headers.env_name(key)
        return @env[name] if @env.key?(name)
        raise ArgumentError, "wrong number of arguments" if defaults.length > 1
        return defaults[0] unless defaults.empty?
        raise KeyError, "key not found: #{key}"
      end

      def self.env_name(key)
        name = key.to_s
        upper = name.upcase
        return upper if upper == "CONTENT_TYPE" || upper == "CONTENT_LENGTH"
        return upper.tr("-", "_") if /\AHTTP_[A-Z0-9_-]+\z/.match?(upper)
        return name unless HTTP_HEADER.match?(name)
        upper = name.upcase.tr("-", "_")
        CGI_VARIABLES.include?(upper) ? upper : "HTTP_" + upper
      end

      # Rails reads Authorization from its ordinary Rack key, legacy
      # server spellings, and the CGI redirect fallback.
      def self.authorization(env)
        headers = Headers.new(env)
        headers["Authorization"] || env["X-HTTP-AUTHORIZATION"] ||
          env["X-HTTP_AUTHORIZATION"] || env["X_HTTP_AUTHORIZATION"] ||
          env["REDIRECT_X_HTTP_AUTHORIZATION"] || headers["X-HTTP-Authorization"]
      end
    end
  end
end
