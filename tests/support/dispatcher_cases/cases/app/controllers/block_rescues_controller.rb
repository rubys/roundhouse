class BlockRescuesController < ApplicationController
  rescue_from ArgumentError do |error|
    render json: { origin: "block", message: error.message }, status: :conflict
  end
  def index
    raise ArgumentError, "original"
  end
end
