class EncodedController < ActionController::Base
  rescue_from ArgumentError do
    payload = JSON.parse('{"text":"<tag>&"}')
    render json: payload, status: :unprocessable_entity
  end
  def index
    raise ArgumentError, "original"
  end
end
