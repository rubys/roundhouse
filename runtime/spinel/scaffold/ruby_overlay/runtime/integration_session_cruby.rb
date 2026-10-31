# frozen_string_literal: true
# `ActionDispatch::Integration::Session` on the ruby tree: the spinel
# twin (runtime/spinel/integration_session.rb) over a Rack env. The
# request takes config.ru's path: a GET or HEAD of a file under public/
# is that file, anything else is `Main.run_rack` in a connection lease.
# The caching middlewares in front of it (GzipCache, ThrusterCache) are a
# proxy's work, which a request from inside the app does not pass.
require "stringio"

module ActionDispatch
  class TestResponse
    attr_reader :status, :body, :headers

    def initialize(status, body, headers)
      @status = status
      @body = body
      @headers = headers
    end
  end

  module Integration
    # Not on Session itself: the test harness reopens Session with its own dispatcher (`include RequestDispatch`), which a later include outranks only over a module.
    module InProcess
      DEFAULT_HOST = "www.example.com"
      DEFAULT_ACCEPT = "text/xml,application/xml,application/xhtml+xml,text/html;q=0.9,text/plain;q=0.8,image/png,*/*;q=0.5"

      attr_reader :host, :response

      def initialize(app)
        @host = DEFAULT_HOST
        @https = false
        @cookies = {}
        @response = nil
      end

      def host!(name)
        @host = name
      end

      def https!(flag = true)
        @https = flag
      end

      def https?
        @https
      end

      def get(path)
        process("GET", path)
      end

      def head(path)
        process("HEAD", path)
      end

      def process(verb, path)
        path_info, _, query = path.partition("?")
        env = {
          "REQUEST_METHOD" => verb,
          "PATH_INFO" => path_info,
          "QUERY_STRING" => query,
          "SERVER_NAME" => @host,
          "SERVER_PORT" => @https ? "443" : "80",
          "HTTP_HOST" => @host,
          "HTTP_ACCEPT" => DEFAULT_ACCEPT,
          "REMOTE_ADDR" => "127.0.0.1",
          "rack.url_scheme" => @https ? "https" : "http",
          "rack.input" => StringIO.new(""),
        }
        if @https
          env["HTTPS"] = "on"
          env["HTTP_X_FORWARDED_PROTO"] = "https"
        end
        env["HTTP_COOKIE"] = @cookies.map { |k, v| "#{k}=#{v}" }.join("; ") unless @cookies.empty?
        saved_request = ActionController::Current.request
        saved_controller = ActionController::Current.controller
        saved_slots = Thread.current[:view_slots]
        saved_broadcasts = Thread.current[:broadcast_log]
        begin
          status, headers, body = answer(verb, path_info, env)
        ensure
          ActionController::Current.request = saved_request
          ActionController::Current.controller = saved_controller
          Thread.current[:view_slots] = saved_slots
          Thread.current[:broadcast_log] = saved_broadcasts
        end
        keep_cookies(Array(headers["set-cookie"]))
        @response = ActionDispatch::TestResponse.new(status, body.join, headers)
        status
      end

      def answer(verb, path_info, env)
        file = verb == "GET" || verb == "HEAD" ? ActionController::PublicFiles.lookup(path_info) : ""
        unless file.empty?
          return Rack::Files.new("public").call(env.merge("PATH_INFO" => file)).then { |s, h, b| [s, h, b.to_a] }
        end
        Db.with_connection { Main.run_rack(env) }
      end

      def keep_cookies(lines)
        lines.each do |line|
          name, value = line.split(";", 2).first.to_s.split("=", 2)
          next if name.to_s.strip.empty?
          if line.downcase.include?("max-age=0")
            @cookies.delete(name.strip)
          else
            @cookies[name.strip] = value.to_s.strip
          end
        end
        nil
      end
    end

    class Session
      include InProcess
    end
  end
end
