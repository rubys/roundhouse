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
# `self.start` is NOT redefined, although that is where `ipaddr:` — the
# keyword campfire's DNS-rebinding pin is written on, and the package
# does not declare — would naturally be added. A reopened, yielding
# CLASS method reached through a scoped constant (`Net::HTTP.start do`)
# mistypes its block parameter as the CALLER's class; `http.request do`
# inside the block then resolves to `Opengraph::Fetch#request` itself,
# the inliner recurses until its rename table fills, and the declined
# call is emitted against a symbol no yielding method has — an
# undefined-symbol link error, with no diagnostic (matz/spinel#4416,
# reduced repro and instrumentation there). Until that lands the
# package's `self.start` is what runs, and spinel drops the undeclared
# `ipaddr:` silently (matz/spinel#4419; the package gap itself is
# matz/spinel#4420, with the block form). Neither is papered over here:
# the pin is not honoured on this lane, and this comment is where that
# is stated.
#
# Last definition wins under spinel, and a redefined method sees the
# class's other methods and ivars — `transport_request` below is the
# package's `#request` body re-stated over `open_connection`/`reconnect`/
# `write_request`/`read_response`, which this file leaves alone.
require "net/http"
require_relative "http_stub"

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
      i = HttpStub.find(req.method, stub_url(req.path))
      res =
        if i < 0
          transport_request(req)
        else
          build_response("1.1", HttpStub::STUB_STATUSES[i].to_s, "", HttpStub.headers_at(i), HttpStub::STUB_BODIES[i])
        end
      blk.call(res) unless blk.nil?
      res
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
