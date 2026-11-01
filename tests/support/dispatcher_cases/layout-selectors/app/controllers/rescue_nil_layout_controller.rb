class RescueNilLayoutController < ActionController::Base
  rescue_from ArgumentError do
    render template: "shared/context", layout: nil
  end
  def index
    raise ArgumentError, "original"
  end
  private
  def selected_layout
    "other"
  end
end
