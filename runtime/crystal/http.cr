# Roundhouse Crystal HTTP runtime.
#
# Hand-written, shipped alongside generated code (copied in by the
# Crystal emitter as `src/http.cr`). Provides the Roundhouse::Http
# surface that emitted controller actions call into: ActionResponse
# (typed return value), ActionContext (params + request shape),
# Router (in-memory route-match table), plus legacy stubs for the
# Phase 4c controller shape (render/redirect_to/head/respond_to)
# that the preview emitters still reference.
#
# Mirrors `runtime/rust/http.rs` / `runtime/typescript/juntos.ts` in
# shape and posture: pure in-process dispatch via `Router.match`
# means tests call controller actions directly — no HTTP server,
# no sockets, no event-loop glue. A real HTTP transport slots in
# later by adding a `HTTP::Handler` on top of the same match table.

module Roundhouse
  module Http
    # What every controller action returns. Fields are optional so
    # actions pick the shape they need:
    #   - `body`: the HTML string the view rendered (for GET actions)
    #   - `status`: HTTP status code (default 200; 422 for
    #     unprocessable, 303 for redirects)
    #   - `location`: redirect target URL; test assertions on
    #     `assert_redirected_to` check this field.
    class ActionResponse
      property body : String
      property status : Int32
      property location : String

      def initialize(@body : String = "", @status : Int32 = 200, @location : String = "")
      end
    end

    # Context passed to every action. `params` merges path params +
    # form body. Values are always strings — controllers coerce to
    # integers via `.to_i64` at the boundary.
    class ActionContext
      getter params : Hash(String, String)

      def initialize(@params : Hash(String, String) = {} of String => String)
      end
    end

    # One entry in the router's match table. The handler is a proc
    # taking ActionContext and returning ActionResponse.
    alias Handler = ActionContext -> ActionResponse

    record Route, method : String, path : String, handler : Handler

    # In-memory router + dispatch. Controllers register routes at
    # require time (the emitted `src/routes.cr` runs Router.root /
    # Router.resources at top level); tests dispatch through
    # `Router.match` without a live HTTP server.
    class Router
      @@routes : Array(Route) = [] of Route

      def self.reset : Nil
        @@routes.clear
      end

      def self.add(method : String, path : String, handler : Handler) : Nil
        @@routes << Route.new(method, path, handler)
      end

      # Match a request path against the registered routes; return
      # the handler + extracted path params, or nil. Path params
      # come from `:id`-style segments in the route pattern.
      def self.match(method : String, path : String) : {Handler, Hash(String, String)}?
        @@routes.each do |route|
          next unless route.method == method
          if extracted = try_match(route.path, path)
            return {route.handler, extracted}
          end
        end
        nil
      end

      private def self.try_match(pattern : String, path : String) : Hash(String, String)?
        pat_parts = pattern.split('/').reject(&.empty?)
        path_parts = path.split('/').reject(&.empty?)
        return nil unless pat_parts.size == path_parts.size
        params = {} of String => String
        pat_parts.zip(path_parts).each do |pat, seg|
          if pat.starts_with?(':')
            params[pat[1..]] = seg
          elsif pat != seg
            return nil
          end
        end
        params
      end
    end

    # Legacy Phase-4c stubs kept for the compile-only pass of
    # controllers still using respond_to/render; pass-2 template
    # actions don't call these.
    class Response
      def initialize
      end
    end

    class Params
      def expect(*args, **kwargs) : Params
        self
      end

      def [](key) : Int64
        0_i64
      end
    end

    def self.params : Params
      Params.new
    end

    def self.render(*args, **kwargs) : Response
      Response.new
    end

    def self.redirect_to(*args, **kwargs) : Response
      Response.new
    end

    def self.head(*args, **kwargs) : Response
      Response.new
    end

    def self.respond_to(&) : Response
      fr = FormatRouter.new
      yield fr
      Response.new
    end

    class FormatRouter
      def html(&) : Response
        yield
        Response.new
      end

      def json(&) : Response
        yield
        Response.new
      end
    end
  end
end
