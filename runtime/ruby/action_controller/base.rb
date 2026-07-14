require_relative "../action_dispatch/flash"
require_relative "../action_dispatch/session"
require_relative "../action_view"

module ActionController
  # Symbolic HTTP statuses used in real-blog. Maps the symbol form
  # (`status: :see_other`) to its integer code. Ad-hoc subset; grow
  # as new statuses surface.
  STATUS_CODES = {
    ok:                    200,
    created:               201,
    accepted:              202,
    no_content:            204,
    moved_permanently:     301,
    found:                 302,
    see_other:             303,
    not_modified:          304,
    bad_request:           400,
    unauthorized:          401,
    forbidden:             403,
    not_found:             404,
    unprocessable_entity:  422,
    # Rails 8.1.x scaffold renamed `:unprocessable_entity` →
    # `:unprocessable_content` mid-version. Alias both so emit follows
    # whichever the fixture's scaffold currently produces.
    unprocessable_content: 422,
    internal_server_error: 500,
  }.freeze

  # Base controller class. Holds the per-request state (params,
  # session, flash) and the response state (status, body, location).
  # Subclasses define their actions and a `process_action` dispatch
  # case (since spinel forbids `send` with non-literal symbols, the
  # action dispatch has to be explicit per-controller).
  #
  # NOTE: `cookies` is intentionally NOT here. It's a CRuby-target
  # feature (used by lobsters, not the blog) provided via the Ruby
  # overlay (runtime/action_controller_cookies.rb), so the shared
  # runtime stays target-agnostic — a CookieJar in this transpiled
  # file would have to satisfy every strict target's type system for
  # a feature none of them exercise yet.
  class Base
    attr_accessor :params, :session, :flash, :request_method, :request_path, :request_format
    attr_reader   :status, :body, :location, :content_type

    def initialize
      @params  = {}
      @session = ActionDispatch::Session.new
      @flash   = ActionDispatch::Flash.new
      @status  = 200
      @body    = ""
      @location = nil
      @request_format = :html
      @content_type = "text/html; charset=utf-8"
      @response_headers = {}
      @performed = false
    end

    # True once render/redirect_to/head has produced a response.
    # The synthesized `process_action` filter preamble checks this
    # after each before_action that can render or redirect — Rails'
    # halting semantics: a filter that responds skips the action.
    def performed?
      @performed
    end

    # Discard the current session (Rails' logout idiom). The dispatch
    # layer persists whatever the session holds after the action; an
    # empty replacement means the outbound session cookie is cleared
    # (or, when a CSRF token is lazily re-added during render, that
    # the next session starts fresh — matching Rails' new-session-id
    # semantics closely enough for cookie-carried state).
    #
    # The trailing `@session` read is load-bearing: assignment is not
    # a value expression on the strict targets (kotlin/swift/C#/rust
    # all reject a `-> Session` body ending in an assignment), so the
    # return must be an explicit read. Same rule as the CookieJar
    # cascade — mutation methods with non-void returns.
    def reset_session
      @session = ActionDispatch::Session.new
      @session
    end

    # Subclasses override. Error message omits `self.class.name` —
    # `.name`-style reflection forks across targets and the runtime
    # stack trace already identifies the receiver's class.
    def process_action(_action_name)
      raise NotImplementedError, "process_action must be overridden by subclass"
    end

    # Render a response. The `content_type` kwarg defaults to the
    # current `@content_type` (`text/html; charset=utf-8` on init).
    # Jbuilder-lowered actions pass `content_type: "application/json"`
    # on the JSON branch; the html branch omits it and rides the
    # default. The `location:` kwarg sets @location so the CGI driver
    # ships a Location header alongside the rendered body — Rails'
    # `render :show, status: :created, location: @article` idiom for
    # POST 201 responses. Distinct from redirect_to (which uses a 3xx
    # status); main.rb dispatches on status, not on @location nil-ness.
    def render(body, status: :ok, content_type: nil, location: nil)
      @body   = body
      @status = resolve_status(status)
      @performed = true
      @content_type = content_type unless content_type.nil?
      @location = location unless location.nil?
      nil
    end

    # `redirect_to(path, notice:, alert:, status:)` — sets location +
    # status; surfaces flash messages via the flash hash. Default
    # status 302 (Found). Real-blog uses 303 (See Other) on
    # PATCH/DELETE responses; pass `status: :see_other` to match.
    def redirect_to(path, notice: nil, alert: nil, status: :found)
      @location = path
      @status   = resolve_status(status)
      @performed = true
      @flash[:notice] = notice unless notice.nil?
      @flash[:alert]  = alert  unless alert.nil?
      nil
    end

    # `head(:no_content, content_type: "application/json")` — empty
    # body, status only. The `content_type` kwarg is set by the
    # respond_to-flattener's JSON branch when it preserves a
    # `head :sym` terminal; html branches omit it and the default
    # text/html stands. (Body-empty responses make Content-Type
    # mostly irrelevant per RFC 7230, but some HTTP clients still
    # parse it, so being explicit costs nothing.)
    def head(status, content_type: nil)
      @status = resolve_status(status)
      @body   = ""
      @performed = true
      @content_type = content_type unless content_type.nil?
      nil
    end

    # `response.headers["Expires"] = …` — Rails actions reach header
    # state through the response object; this controller IS its own
    # buffered response, so `response` returns self and `headers` the
    # extra-header hash. The CGI harness emits status/body/
    # content-type today; extra headers are buffered but unsent — a
    # ledgered seam (they tune caching, not content), wired through
    # the harness when a consumer needs them.
    def response
      self
    end

    def headers
      @response_headers
    end

    # `send_data data, type:, disposition:` — a binary response body
    # (lobsters streams avatar PNGs). Same buffering contract as
    # render.
    def send_data(data, type: "application/octet-stream", disposition: "attachment")
      @body = data
      @content_type = type
      @response_headers["Content-Disposition"] = disposition
      @performed = true
      nil
    end

    # Monomorphic on Symbol — real-blog never passes a literal Integer
    # status, so the previous `is_a?(Integer)` pass-through branch is
    # contracted away. Symbol -> Integer via the STATUS_CODES table.
    def resolve_status(s)
      STATUS_CODES.fetch(s, 200)
    end
  end
end
