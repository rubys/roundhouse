require "json"
# Tep::Request -- what the handler reads off the wire.
module Tep
  class Request
    attr_accessor :verb, :path, :raw_path, :http_version
    # `req_params` (not `params`): renamed to avoid sharing an ivar
    # slot with the controller's `@params`. Roundhouse assigns the
    # controller a nested poly hash (params["article"]["title"]); under
    # spinel two classes sharing an ivar name unify its type, so a
    # plain `@params` here would be widened to that poly hash and break
    # this class's String->String reads. Same fix as req_headers.
    attr_accessor :req_params, :query, :req_headers, :raw_body, :cookies
    # The query string as it arrived, and a multipart body's text fields
    # by name: what `Main.request_params` builds the controller's params
    # from with Rails' own rules (runtime/param_builder.rb). The flat
    # hashes above keep only a repeated key's last value, and cannot say
    # `ids[]=1&ids[]=2`.
    attr_accessor :raw_query, :body_fields, :json_body_params
    attr_accessor :remote_host
    attr_accessor :ivars

    def initialize
      @verb         = +""
      @path         = +""
      @raw_path     = +""
      @http_version = "HTTP/1.0"
      @req_params       = Tep.str_hash   # path captures + query + form merged
      @query        = Tep.str_hash   # raw query string only
      @raw_query    = +""
      @body_fields  = Tep.str_hash
      # JSON request params keep their decoded Ruby values. Form and
      # multipart text fields stay in the String-only `body_fields` map;
      # multipart files stay in `uploads` below.
      @json_body_params = {}
      @req_headers  = Tep.str_hash   # downcased header names; renamed
                                     # from `headers` to avoid sharing
                                     # an ivar slot with Response (spinel
                                     # mis-codegens polymorphic ivar
                                     # writes when two classes share an
                                     # ivar name).
      @cookies      = Tep.str_hash   # parsed from Cookie: header
      # @session dropped: collides with controllers' :session via
      # poly-receiver dispatch. Roundhouse uses ActionDispatch::Session.
      @raw_body     = +""             # same reasoning as req_headers
      @remote_host  = +""
      @passed       = false          # `pass` flag: skip to the next matching route
      @ivars        = Tep.str_hash   # per-request bag for `@name = ...`
                                     # set by handlers and `before` filters,
                                     # read by templates as `ivars[k]`. The
                                     # Sinatra-compat translator rewrites
                                     # `@x = v` -> `req.ivars["x"] = (v).to_s`
                                     # in handler bodies and `@x` -> `ivars["x"]`
                                     # inside ERB chunks.
      # File parts of a multipart body, by full field name
      # (`message[attachment]`); the text parts join @req_params. The
      # typed String hash cannot hold a file, so the two travel apart
      # and `Main.request_params` / `Main.nest_uploads` fold both into the
      # controller's params.
      @uploads      = {}
    end

    attr_accessor :passed
    attr_accessor :uploads
    def set_passed; @passed = true; end

    # Sinatra-compat read aliases removed in the vendored copy.
    # `req.body` collides with controller#body via spinel poly dispatch
    # in the roundhouse build; `req.raw_body` is the canonical name.

    # Spinel's Hash[k] returns "" for missing string keys, not nil --
    # so an empty Connection header looks the same as no header at all.
    # We treat both as "use HTTP/1.1 default behaviour".
    def keep_alive?
      lc = @req_headers["connection"].downcase
      if lc == "close"
        return false
      end
      if lc == "keep-alive"
        return true
      end
      @http_version == "HTTP/1.1"
    end

    # The declared body length; -1 when the header is not a plain decimal
    # byte count (see Tep.decimal_byte_count). The drains below only ever
    # see a value `body_refusal` passed, but -1 reads nothing there too.
    def content_length
      Tep.decimal_byte_count(@req_headers["content-length"])
    end

    # What the server must answer instead of reading this request's body:
    # 400 for unsupported Transfer-Encoding or an invalid Content-Length,
    # 413 for a length past `max`, 0 to go ahead. Decided from the headers alone, so a refused
    # body is never read — each server asks this BEFORE its drain, since
    # the drain itself is what held the bytes.
    def body_refusal(max)
      # There is no request chunk decoder. Even an empty field is a
      # Transfer-Encoding, so check presence, not its string value.
      if @req_headers.key?("transfer-encoding")
        return 400
      end
      cl = content_length
      if cl < 0
        return 400
      end
      # A saturated length is "too large to represent", refused whatever
      # `max` is — never compared as a size (see Tep.decimal_byte_count).
      if cl >= Tep::BYTE_COUNT_CEILING || cl > max
        return 413
      end
      0
    end

    def form?
      @req_headers["content-type"].downcase.start_with?("application/x-www-form-urlencoded")
    end

    # True when the request body is a multipart/form-data submission
    # (browsers use this for any form built via `new FormData(...)`
    # or carrying file inputs). Parsed by the ruby family's
    # `ActionDispatch::Http::Multipart` (runtime/multipart.rb): text
    # parts into @req_params, file parts into @uploads.
    def multipart?
      @req_headers["content-type"].downcase.start_with?("multipart/form-data")
    end

    # True for a JSON body — what `@rails/request.js` sends when a
    # controller asks for `contentType: "application/json"` (campfire's
    # link unfurl posts `{"url": …}` this way). Rails parses it into
    # params; `parse_body_params` does the same below.
    def json?
      @req_headers["content-type"].downcase.start_with?("application/json")
    end

    # ---- Rack::Request-style accessors (reads only, no .ip yet) ----
    # These are convenience getters over headers we already parse;
    # `.ip` would need a sphttp_accept_with_peer C helper before it
    # can land cleanly, so it's deferred.

    def host;          @req_headers["host"];        end
    def user_agent;    @req_headers["user-agent"];  end
    def referer;       @req_headers["referer"];     end
    def referrer;      @req_headers["referer"];     end   # spelling alias
    def accept;        @req_headers["accept"];      end
    def content_type;  @req_headers["content-type"]; end

    # tep doesn't terminate TLS itself; both flags reflect "is this
    # connection encrypted from the client's view?" via the
    # `X-Forwarded-Proto: https` header that any reasonable reverse
    # proxy sets.
    def scheme
      proto = @req_headers["x-forwarded-proto"]
      if proto.length > 0
        return proto.downcase
      end
      "http"
    end

    def ssl?
      scheme == "https"
    end

    # Pull any remaining body bytes from `client_fd` up to the
    # advertised Content-Length, then merge form fields into @req_params.
    # Returns false on an incomplete body, true after a complete one.
    # Called once per request by the server right after Parser.parse
    # populates the request headers + the body bytes already in the
    # recv buffer.
    #
    # No-op on bodyless requests. Form parsing handles
    # `application/x-www-form-urlencoded` and `multipart/form-data`
    # (see `parse_body_params`); JSON stays in @json_body_params and
    # @raw_body stays intact either way.
    def consume_body(client_fd)
      cl = content_length
      # bytesize, not length: Content-Length counts bytes, and a body
      # carrying multibyte UTF-8 (a boost emoji, a multipart field)
      # reads short on characters — leaving the tail of the body unread
      # on the socket, where it corrupts the next keep-alive request.
      already = @raw_body.bytesize
      if cl > already
        rest = Sock.sphttp_drain_body(client_fd, cl - already)
        @raw_body = @raw_body + rest
      end
      # RFC 9112, 6.3 rule 6: never parse or dispatch a partial body.
      if @raw_body.bytesize < cl
        return false
      end
      parse_body_params
      true
    end

    # The body's form fields into @req_params: urlencoded pairs, or the
    # text and file parts of a multipart body. JSON is kept separately
    # in @json_body_params so its values retain their JSON types and do
    # not participate in request-method override. Shared by the three
    # drains below, which differ only in how they wait for bytes.
    def parse_body_params
      if form?
        Url.parse_query(@raw_body).each do |k, v|
          @req_params[k] = v
        end
      elsif multipart?
        form = ActionDispatch::Http::Multipart.parse(@raw_body, @req_headers["content-type"])
        form.fields.each do |k, v|
          @req_params[k] = v
          @body_fields[k] = v
        end
        form.files.each do |k, v|
          @uploads[k] = v
        end
      elsif json? && @raw_body.length > 0
        # Rails answers a malformed body 400 before the action runs;
        # here the params stay empty and the action's own `require`
        # refuses it.
        begin
          parsed = JSON.parse(@raw_body)
          if parsed.is_a?(Hash)
            parsed.each do |k, v|
              @json_body_params[k.to_s] = v
            end
          else
            # ActionDispatch wraps top-level arrays and scalars under
            # `_json`, keeping request_parameters a Hash.
            @json_body_params["_json"] = parsed
          end
        rescue JSON::ParserError
          nil
        end
      end
      nil
    end

    # Scheduler-friendly body drain, used by Tep::Server::Scheduled (the
    # TEP_SERVER=fiber measurement lane, whose client fd is non-blocking
    # -- a blocking recv would starve the whole loop). Loops on
    # Sock.sphttp_recv_some + Tep::Scheduler.io_wait so other fibers keep
    # running while we wait for body bytes; a 5s per-recv timeout drops a
    # client that opened the request but never sent the body. Form parse
    # mirrors consume_body.
    def consume_body_via_scheduler(client_fd)
      cl = content_length
      while @raw_body.bytesize < cl
        ready = Tep::Scheduler.io_wait(client_fd, Tep::Scheduler::READ, 5)
        if ready == 0
          return false   # timeout -- client never finished sending
        end
        chunk = Sock.sphttp_recv_some(client_fd, cl - @raw_body.bytesize)
        if chunk.bytesize == 0
          return false   # peer closed mid-body
        end
        @raw_body = @raw_body + chunk
      end
      # RFC 9112, 6.3 rule 6: never parse or dispatch a partial body.
      if @raw_body.bytesize < cl
        return false
      end
      parse_body_params
      true
    end

    # Body drain for Tep::Server::Threaded, whose client fd is
    # non-blocking: `io` is the connection's `IO.for_fd` wrapper, and its
    # timed `wait_readable` parks this green thread until body bytes
    # arrive; a 5s per-recv timeout drops a client that opened the
    # request but never sent the body. Form parse mirrors consume_body.
    def consume_body_via_io(io, client_fd)
      cl = content_length
      # bytesize throughout — same reasoning as consume_body above; on
      # this path the char/byte gap also cost a 5s wait per multibyte
      # body, waiting for bytes that had already arrived.
      while @raw_body.bytesize < cl
        ready = io.wait_readable(5)
        if ready.nil?
          return false   # timeout -- client never finished sending
        end
        chunk = Sock.sphttp_recv_some(client_fd, cl - @raw_body.bytesize)
        if chunk.bytesize == 0
          return false   # peer closed mid-body
        end
        @raw_body = @raw_body + chunk
      end
      # RFC 9112, 6.3 rule 6: never parse or dispatch a partial body.
      if @raw_body.bytesize < cl
        return false
      end
      parse_body_params
      true
    end
  end
end
