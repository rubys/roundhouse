class FalseLayoutController < ActionController::Base
  before_action { render template: "shared/context", layout: false }
  def index
    raise "action ran"
  end
end
