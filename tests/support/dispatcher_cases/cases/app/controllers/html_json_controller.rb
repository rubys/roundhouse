class HtmlJsonController < ActionController::Base
  before_action { render json: { origin: "html-before" }, status: :forbidden }
  def index
    raise "action ran"
  end
end
