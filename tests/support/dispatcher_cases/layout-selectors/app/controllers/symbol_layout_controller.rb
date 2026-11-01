class SymbolLayoutController < ActionController::Base
  before_action { render template: "shared/context", layout: :other }
  def index
    raise "action ran"
  end
end
