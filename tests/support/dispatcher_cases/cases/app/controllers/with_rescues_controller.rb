class WithRescuesController < ApplicationController
  rescue_from ArgumentError, with: :report_error
  def index
    raise ArgumentError, "original"
  end
  private
  def report_error(error)
    render json: { origin: "named", message: error.message }, status: :conflict
  end
end
