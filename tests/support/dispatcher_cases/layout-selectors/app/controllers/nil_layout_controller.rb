class NilLayoutController < ActionController::Base
  before_action { render template: "shared/context", layout: nil }
  def index
    raise "action ran"
  end
end
