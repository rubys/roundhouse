class RescuedContextController < ActionController::Base
  rescue_from ArgumentError do
    render template: "shared/context", layout: false, status: :conflict
  end
  def index
    raise ArgumentError, "original"
  end
end
