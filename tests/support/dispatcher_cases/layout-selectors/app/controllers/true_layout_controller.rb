class TrueLayoutController < ActionController::Base
  layout "application"
  before_action { render template: "shared/context", layout: true }
  def index
    raise "action ran"
  end
end
