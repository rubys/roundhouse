# Net::HTTP, reopened over spinel's bundled `packages/net` so the stub
# table (`runtime/http_stub.rb`) is consulted BEFORE any socket opens.
#
# THE RUBY FAMILY NEVER LOADS THIS FILE. `project::ruby_runtime_files`
# swaps it for a bare `require "net/http"`: over there the real WebMock
# gem intercepts CRuby's own client, and a second `Net::HTTP` beside it
# would be a reopen of the wrong class. Same arrangement as `resolv.rb`.
#
# WHY A REOPEN AND NOT A SEPARATE DOUBLE. `Opengraph::Fetch` and
# `Webhook` are written against `Net::HTTP` by name — `Net::HTTP.start`,
# `Net::HTTP::Get.new`, `is_a?(Net::HTTPRedirection)`, `rescue
# Net::OpenTimeout` — so the double has to BE that constant. Reopening
# the package's class keeps every one of those names real (the response
# hierarchy in particular: `is_a?` on a bare struct answers nothing),
# and keeps the real transport reachable for a request no stub answers.
# Webhook delivery works on the spinel binary today through this
# class; a double that replaced it would have taken that away.
#
# WHAT IS REDEFINED, AND WHAT DELIBERATELY IS NOT.
#
# `#request` carries the stub lookup. It does NOT (yet) gain the block
# form the package lacks (matz/spinel#4420; measured 2026-09-10:
# `http.request(req) { |res| }` compiled and the block never ran — no
# error, no warning — so campfire's unfurl spun through MAX_REDIRECTS
# and raised TooManyRedirectsError): see the note on the method for
# what a yielding `request` cost the block-less callers.
#
# `#start` (the INSTANCE method) becomes lazy: it marks the client
# started and opens nothing. The package's `self.start` connects
# eagerly — `http.start` runs BEFORE the block is yielded — which would
# put a real TLS connect to www.example.com ahead of any stub. Deferring
# the connect to the first request that no stub answers is what puts
# the double above the transport.
#
# `self.start` is NOT redefined, and `ipaddr:` — the keyword campfire's
# DNS-rebinding pin is written on — is the PACKAGE's to honour: since
# matz/spinel#4420 closed (2026-09-10) `Net::HTTP.start` declares it and
# `open_connection` connects to it while `Host:` and the TLS name stay
# `address`. Probed 2026-09-17: `Net::HTTP.start("example.com", 80,
# ipaddr: "127.0.0.1")` from a spinel binary fails with ECONNREFUSED on
# 127.0.0.1, exactly as CRuby does, so the pin holds on this lane. The
# TEST's proof of it — `TCPSocket.expects(:open).with { … }.throws` —
# is served by `connect_with_timeout` below asking `TcpSocketStub` about
# the address it is about to connect to, which is where CRuby's mocha
# would have intercepted `TCPSocket.open`. (#4416 and #4419, the
# reopened-yielding-class-method and dropped-keyword bugs this comment
# used to describe, closed the same day.)
#
# Last definition wins under spinel, and a redefined method sees the
# class's other methods and ivars — `transport_request` below is the
# package's `#request` body re-stated over `open_connection`/`reconnect`/
# `write_request`/`read_response`, which this file leaves alone.
require "net/http"
require_relative "http_stub"
require_relative "tcp_socket_stub"

module Net
  class HTTPResponse
    # The slice a streaming reader takes at a time. `Opengraph::Fetch`
    # bails when the accumulated body crosses MAX_BODY_SIZE, and three
    # of its tests exist only to exercise that path — a `read_body` that
    # handed the whole String over in one yield would pass the happy
    # path and silently break all three.
    READ_BODY_CHUNK = 16384

    # `response.read_body { |chunk| ... }` — CRuby streams the body from
    # the socket here; this package has already read it whole, so the
    # stream is the String, sliced. A declared `&blk`, not `yield`, for
    # the reason `HTTP#request` below gives: the response reaches
    # `Opengraph::Fetch#size_restricted_body` boxed, and a yielding
    # method has no dispatch entry for a boxed receiver — the call
    # landed on nothing and raised `no block given (yield)`.
    def read_body(&blk)
      unless blk.nil?
        offset = 0
        total = @body.bytesize
        while offset < total
          blk.call(@body.byteslice(offset, READ_BODY_CHUNK).to_s)
          offset += READ_BODY_CHUNK
        end
      end
      @body
    end

    # Integer or nil, as CRuby answers it — `Opengraph::Fetch` applies
    # `.to_i` and compares, so nil has to survive the read.
    def content_length
      v = @headers["content-length"]
      v.nil? ? nil : v.to_i
    end
  end

  class HTTP
    # Lazy: started, but with no socket until a request needs one. The
    # package's `finish` closes whatever is open and clears the flags,
    # so a `start`/`finish` pair around nothing but stubbed requests
    # touches no descriptor at all.
    def start
      @started = true
      self
    end

    # The URL this connection would put on the wire for `path`, in the
    # spelling `HttpStub.normalize` files stubs under.
    def stub_url(path)
      scheme = @use_ssl ? "https" : "http"
      default = @use_ssl ? 443 : 80
      host = @address.to_s.downcase
      authority = @port == default ? host : "#{host}:#{@port}"
      "#{scheme}://#{authority}#{path}"
    end

    # `http.request(req)`. A stub answers without a connection; otherwise
    # the package's own path runs. The response class comes from the
    # package's `build_response`, so a stubbed 302 IS a
    # `Net::HTTPRedirection` and a stubbed 200 a `Net::HTTPOK`.
    #
    # The block form is a declared `&blk` CALLED, not a `yield` — the
    # spelling the package itself settled on for matz/spinel#4420. A
    # method that yields is inlined at its call sites and has no entry
    # in spinel's dynamic dispatch, and `Webhook#http` comes back BOXED
    # (its return is emitted `sp_box_nullable_obj`), so a yielding
    # `request` left `http.request(post)` there with no arm at all:
    # `undefined method 'request' for an instance of Net::HTTP` in all
    # four delivery tests. A `&blk` parameter keeps the standalone
    # entry, so the block-less webhook call and `Opengraph::Fetch`'s
    # `request(req) { |res| … }` dispatch to the same method. The
    # response is complete before the block sees it, as the package
    # documents; `read_body` above is what slices it for a streaming
    # reader.
    def request(req, &blk)
      i = HttpStub.find_for(req.method, stub_url(req.path), req.body)
      res =
        if i < 0
          transport_request(req)
        else
          build_response("1.1", HttpStub::STUB_STATUSES[i].to_s, "", HttpStub.headers_at(i), HttpStub::STUB_BODIES[i])
        end
      blk.call(res) unless blk.nil?
      res
    end

    # The package's `connect_with_timeout`, re-stated over one question
    # to the socket seam: `TcpSocketStub.check` on the address this
    # connection is about to open — `ipaddr:` when the caller pinned one,
    # the hostname otherwise — and the port. With nothing filed it
    # answers nil and the connect proceeds exactly as the package wrote
    # it; a test's `TCPSocket.expects(:open)` throws or fails here, the
    # way mocha's replacement of `TCPSocket.open` does under CRuby.
    def connect_with_timeout
      limit = @open_timeout.nil? ? 0 : @open_timeout
      target = @ipaddr.empty? ? @address : @ipaddr
      TcpSocketStub.check(target, @port)
      return TCPSocket.new(target, @port) if limit <= 0
      s = Socket.new(Socket::AF_INET, Socket::SOCK_STREAM, 0)
      begin
        s.connect_nonblock(target, @port)
      rescue IO::WaitWritable
        if IO.select(nil, [s], nil, limit).nil?
          s.close
          raise OpenTimeout
        end
        begin
          s.connect_nonblock(target, @port)
        rescue Errno::EISCONN
          # already connected: the wait above is what completed it
        end
      end
      s
    end

    # The package's `#request`, re-stated: a request on an unstarted
    # client opens and closes around itself; a started one opens its
    # socket on first use (the lazy `start` above) and reconnects after
    # the `Connection: close` every response carries.
    def transport_request(req)
      unless @started
        begin
          @started = true
          return transport_request(req)
        ensure
          finish
        end
      end
      if @socket.nil?
        open_connection
      elsif !@fresh
        reconnect
      end
      @fresh = false
      write_request(req)
      read_response
    end
  end
end

module Net
  class HTTP
    # net-http-persistent's client, as far as campfire reaches it:
    # `WebPush::Pool` builds one per process (`name:`, `pool_size:`),
    # hands it to every delivery as `connection:`, and shuts it down at
    # exit; the gem's own `WebPush::Request` then calls `request(uri,
    # req)` on it. The ruby family has the real gem (`RUNTIME_GEMS`).
    #
    # NOTHING IS KEPT ALIVE HERE, and that is spinel's client rather
    # than a choice of this file's: the package speaks `Connection:
    # close` on every response (see `transport_request` above), so a
    # "persistent" connection on this lane is a client opened per
    # request. The pool-size and name are accepted and unused. What is
    # honoured is the interface — a caller holding one of these can
    # send a request through it and get the package's response back,
    # stub table included.
    class Persistent
      def initialize(name: nil, pool_size: nil)
        @name = name.to_s
      end

      def name
        @name
      end

      def request(uri, req)
        http = Net::HTTP.new(uri.host, uri.port)
        http.use_ssl = (uri.scheme == "https")
        http.request(req)
      end

      def shutdown
        nil
      end
    end
  end
end
