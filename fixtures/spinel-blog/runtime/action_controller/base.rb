require_relative "parameters"
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
    internal_server_error: 500,
  }.freeze

  # Base controller class. Holds the per-request state (params,
  # session, flash) and the response state (status, body, location).
  # Subclasses define their actions and a `process_action` dispatch
  # case (since spinel forbids `send` with non-literal symbols, the
  # action dispatch has to be explicit per-controller).
  class Base
    attr_accessor :params, :session, :flash, :request_method, :request_path
    attr_reader   :status, :body, :location

    def initialize
      @params  = ActionController::Parameters.new({})
      @session = {}
      @flash   = {}
      @status  = 200
      @body    = ""
      @location = nil
    end

    # Subclasses override.
    def process_action(_action_name)
      raise NotImplementedError, "#{self.class.name} must override process_action"
    end

    # Render an HTML response. Two argument shapes accepted:
    #   render(html_string, status: 200)
    #   render(status: 422)   — re-renders the implicit action; not used
    #                           in spinel-blog because actions render
    #                           their views explicitly.
    def render(html, status: 200)
      @body   = html
      @status = resolve_status(status)
      nil
    end

    # `redirect_to(path, notice:, alert:, status:)` — sets location +
    # status; surfaces flash messages via the flash hash. Default
    # status 302 (Found). Real-blog uses 303 (See Other) on
    # PATCH/DELETE responses; pass `status: :see_other` to match.
    def redirect_to(path, notice: nil, alert: nil, status: :found)
      @location = path
      @status   = resolve_status(status)
      @flash[:notice] = notice unless notice.nil?
      @flash[:alert]  = alert  unless alert.nil?
      nil
    end

    # `head(:no_content)` etc. — empty body, status only.
    def head(status)
      @status = resolve_status(status)
      @body   = ""
      nil
    end

    def resolve_status(s)
      return s if s.is_a?(Integer)
      STATUS_CODES.fetch(s, 200)
    end
  end
end
