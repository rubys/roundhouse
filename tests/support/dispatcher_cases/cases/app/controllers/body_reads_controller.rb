class BodyReadsController < ApplicationController
  after_action { response.set_header("X-Observed", response.body) }
  def index
    render json: { origin: "action" }, status: :accepted
  end
end
