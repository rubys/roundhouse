class DynamicLayoutController < ActionController::Base
  before_action { render template: "shared/context", layout: selected_layout }
  def index
    raise "action ran"
  end
  private
  def selected_layout
    "other"
  end
end
