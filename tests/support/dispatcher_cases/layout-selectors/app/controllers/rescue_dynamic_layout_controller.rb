class RescueDynamicLayoutController < ActionController::Base
  rescue_from ArgumentError do
    render template: "shared/context", layout: selected_layout
  end
  def index
    raise ArgumentError, "original"
  end
  private
  def selected_layout
    "other"
  end
end
