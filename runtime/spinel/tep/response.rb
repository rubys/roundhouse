# Tep::Response -- what the handler writes back. Headers are a Bag
# (string-keyed); the framework adds Content-Length / Connection
# automatically when serializing.
module Tep
  class Response
    attr_accessor :status, :headers, :body, :halted, :file_path, :set_cookies

    def initialize
      @status      = 200
      @headers     = Tep.str_hash
      @body        = +""
      @halted      = false
      @file_path   = +""
      # `Set-Cookie` is a header that can repeat; can't shove multiple
      # values into a Hash slot. Each entry here is one fully-formatted
      # Set-Cookie line, emitted verbatim by the writer.
      @set_cookies = [""]
      # pop, not clear/delete_at: the type-seed removal idiom hits a
      # per-(method x representation) arm matrix in spinel's shared-
      # string machinery (matz/spinel#3306) -- pop and shift are armed
      # on every representation this ivar takes across both entry
      # points (main.rb / bin/blog.rb); clear and delete_at each have
      # an unarmed cell.
      @set_cookies.pop
      @streamer    = Streamer.new   # default no-op; only used when @streaming
      @streaming   = false
      # WebSocket upgrade slots. When @upgrading_ws is set (by
      # start_websocket, from the /cable handler), Tep::Server::Scheduled's
      # write path emits the 101 Switching Protocols handshake and then
      # drives Tep::WebSocket::Connection's recv loop instead of writing
      # a normal body.
      @upgrading_ws  = false
      @ws_accept_key = +""
      @ws_driver     = Tep::WebSocket::Driver.new(0)
    end

    attr_accessor :streamer, :streaming
    attr_accessor :upgrading_ws, :ws_accept_key, :ws_driver

    def start_stream(streamer)
      @streamer  = streamer
      @streaming = true
    end

    # Mark the response as a WebSocket upgrade. The server writes a
    # 101 Switching Protocols response with the accept-key, assigns
    # the live client fd onto the driver, then runs the recv loop.
    def start_websocket(accept_key, driver)
      @upgrading_ws  = true
      @ws_accept_key = accept_key
      @ws_driver     = driver
    end

    # Sinatra-style cookie writer. `opts` is a Bag-of-strings
    # (path, expires, max-age, domain, samesite, httponly, secure).
    # Empty `opts` is fine: just writes "name=value".
    def set_cookie(name, value, opts)
      line = name + "=" + Url.escape(value)
      if opts.length > 0
        opts.each do |k, v|
          if v.length == 0
            line << "; " + k          # bare flag (HttpOnly, Secure)
          else
            line << "; " + k + "=" + v
          end
        end
      end
      @set_cookies.push(line)
    end

    def send_file(path)
      @file_path = path
      @body = +""
    end

    # Spinel's polymorphic-receiver write codegen emits a no-op for
    # `res.body = x` when called from a context that has a poly
    # value, so we force the assignment through this method (where
    # `self` is unambiguously Response).
    def set_body_if_empty(s)
      if @body.length == 0 && s.length > 0
        @body = s
      end
    end

    def set_status(n); @status = n; end

    def halted_close?
      @halted && @status >= 300
    end
  end
end
