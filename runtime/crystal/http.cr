# Roundhouse Crystal HTTP runtime — Phase 4c compile-only stubs.
#
# Hand-written, shipped alongside generated code (copied in by the
# Crystal emitter as `src/http.cr`). Provides just enough surface that
# emitted controller actions type-check: `Response`, a `Params`
# placeholder, and the free methods generated code expects
# (`render`, `redirect_to`, `head`, `respond_to`).
#
# Mirrors `runtime/rust/http.rs` one-for-one — same stubs, same
# behavior (every call returns `Response.new`). Real behavior lands in
# Phase 4e+. Controller tests stay `pending` until then, so nothing in
# this module actually executes during `crystal spec`; the purpose is
# to make `crystal build` succeed.

module Roundhouse
  module Http
    # Opaque response value. The real runtime will carry status + body
    # + headers; Phase 4c only needs a value every action can return.
    class Response
      def initialize
      end
    end

    # Controller-side view of request parameters. Bare `params` in a
    # Ruby controller lowers to `Roundhouse::Http.params` — both reads
    # and the `params.expect(...)` surface live on this stub.
    class Params
      # `params.expect(:key)` / `params.expect(article: [:title, :body])`.
      # Accepts any positional or keyword shape; returns `self` so
      # chained helpers (`Article.new(params.expect(...))`) compile.
      # The real runtime will type-check and coerce from the request.
      def expect(*args, **kwargs) : Params
        self
      end

      # `params[:id]` / `params["id"]`. Returns a zero Int64 — Phase
      # 4c's most common call site is `Model.find(params[:id])`, whose
      # argument type is `Int64`. String-indexed accesses will need a
      # different overload when we add one.
      def [](key) : Int64
        0_i64
      end
    end

    # Accessor emitted for a bare `params` reference in a controller
    # body.
    def self.params : Params
      Params.new
    end

    # Stub `render`. Accepts any positional + keyword arg shape the
    # emitter produces (template symbol, string, or an options hash).
    def self.render(*args, **kwargs) : Response
      Response.new
    end

    # Stub `redirect_to`. First positional arg is the target (a model,
    # a string URL, a path helper result); kwargs carry the Rails
    # options (`notice:`, `status:`, etc.).
    def self.redirect_to(*args, **kwargs) : Response
      Response.new
    end

    # Stub `head :status`. Emitted when an action returns only a status.
    def self.head(*args, **kwargs) : Response
      Response.new
    end

    # `respond_to do |format| ... end` lowers to this: the block gets a
    # `FormatRouter` and the caller threads the format-specific
    # `Response` back out. Phase 4c wires only the HTML branch; the
    # JSON branch is replaced at the call site with a `# TODO: JSON
    # branch` comment.
    def self.respond_to(&) : Response
      fr = FormatRouter.new
      yield fr
      Response.new
    end

    class FormatRouter
      # HTML branch. Runs the block for its side effects and surfaces
      # the Response.
      def html(&) : Response
        yield
        Response.new
      end

      # JSON branch stub — Phase 4c replaces callers with a
      # `# TODO: JSON branch` comment; kept callable so hand-written
      # code outside the emitter still typechecks.
      def json(&) : Response
        yield
        Response.new
      end
    end
  end
end
