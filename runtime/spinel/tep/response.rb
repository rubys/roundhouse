# Tep::Response -- what the handler writes back. Headers are a Bag
# (string-keyed); the framework adds Content-Length / Connection
# automatically when serializing.
module Tep
  class Response
    attr_accessor :status, :headers, :body, :halted, :file_path, :set_cookies

    def initialize
      @status      = 200
      @headers     = Tep.str_hash
      @body        = ""
      @halted      = false
      @file_path   = ""
      # `Set-Cookie` is a header that can repeat; can't shove multiple
      # values into a Hash slot. Each entry here is one fully-formatted
      # Set-Cookie line, emitted verbatim by the writer.
      @set_cookies = [""]
      @set_cookies.delete_at(0)
      @streamer    = Streamer.new   # default no-op; only used when @streaming
      @streaming   = false
    end

    attr_accessor :streamer, :streaming

    def start_stream(streamer)
      @streamer  = streamer
      @streaming = true
    end

    # Sinatra-style cookie writer. `opts` is a Bag-of-strings
    # (path, expires, max-age, domain, samesite, httponly, secure).
    # Empty `opts` is fine: just writes "name=value".
    def set_cookie(name, value, opts)
      line = name + "=" + Url.escape(value)
      if opts.length > 0
        opts.each do |k, v|
          if v.length == 0
            line = line + "; " + k          # bare flag (HttpOnly, Secure)
          else
            line = line + "; " + k + "=" + v
          end
        end
      end
      @set_cookies.push(line)
    end

    def send_file(path)
      @file_path = path
      @body = ""
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
