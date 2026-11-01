class ApplicationLayoutController < ActionController::Base
  before_action { render template: "shared/context", layout: "application" }
  def index
    raise "action ran"
  end
end
