class RescuePagesController < ActionController::Base
  rescue_from ArgumentError do
    render template: "pages/blocked", layout: false, status: :conflict
  end
  def index
    raise ArgumentError, "original"
  end
end
