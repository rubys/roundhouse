# frozen_string_literal: true
# `ActionDispatch::Integration::Session` (actionpack 8.1.4) on spinel: a
# request answered by the app itself, in process, through the same
# `Main.dispatch` the server calls. lobsters' CachePageJob replays a page
# this way and writes the body to the page cache.
#
# As Rails' session does, it starts at `www.example.com` over http, sends
# Rails' default Accept and `127.0.0.1` as the remote address, and keeps
# the cookies a response sets for the next request.
#
# The request runs on the calling thread, so the thread's request state
# (`ActionController::Current`, the view slots, the broadcast log) is put back afterwards: a
# job run inline inside a request goes on rendering that request.
#
# NOT HERE: verbs other than GET and HEAD, `params:`/`headers:`/`env:`,
# and following redirects. The ruby tree's twin is
# runtime/integration_session_cruby.rb.
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

      def initialize(app)
        @host = DEFAULT_HOST
        @https = false
        @cookies = {}
        @response = nil
      end

      def host
        @host
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

      def response
        @response
      end

      def get(path)
        process("GET", path)
      end

      def head(path)
        process("HEAD", path)
      end

      def process(verb, path)
        req = Tep::Request.new
        req.verb = verb
        req.raw_path = path
        qmark = path.index("?")
        if qmark.nil?
          req.path = path
        else
          req.path = path[0, qmark]
          query = path[qmark + 1, path.length - qmark - 1]
          req.query = Tep::Url.parse_query(query)
          req.raw_query = query
        end
        req.query.each { |k, v| req.req_params[k] = v }
        req.req_headers["host"] = @host
        req.req_headers["accept"] = DEFAULT_ACCEPT
        req.req_headers["x-forwarded-proto"] = "https" if @https
        req.remote_host = "127.0.0.1"
        unless @cookies.empty?
          req.req_headers["cookie"] = @cookies.map { |k, v| k + "=" + v }.join("; ")
          @cookies.each { |k, v| req.cookies[k] = Tep::Url.unescape(v) }
        end
        res = Tep::Response.new
        saved_request = ActionController::Current.request
        saved_controller = ActionController::Current.controller
        saved_slots = Thread.current[:view_slots]
        saved_broadcasts = Thread.current[:broadcast_log]
        begin
          Db.with_connection { Main.dispatch(req, res) }
        ensure
          ActionController::Current.request = saved_request
          ActionController::Current.controller = saved_controller
          Thread.current[:view_slots] = saved_slots
          Thread.current[:broadcast_log] = saved_broadcasts
        end
        keep_cookies(res.set_cookies)
        # Not res.body: a public/ file is left to the server to send, so read it as the response the client would get.
        body = res.file_path.empty? || verb == "HEAD" ? res.body : File.read(res.file_path)
        @response = ActionDispatch::TestResponse.new(res.status, body, res.headers)
        res.status
      end

      # Each line is one `Set-Cookie`: `name=value; Path=/; …`; an expired one removes the name.
      def keep_cookies(lines)
        lines.each do |line|
          pair = line.split(";")[0].to_s
          eq = pair.index("=")
          next if eq.nil? || eq == 0
          name = pair[0, eq].strip
          if line.downcase.include?("max-age=0")
            @cookies.delete(name)
          else
            @cookies[name] = pair[eq + 1, pair.length - eq - 1].strip
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
