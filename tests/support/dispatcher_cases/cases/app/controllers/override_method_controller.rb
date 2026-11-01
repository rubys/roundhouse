class OverrideMethodController < WithRescuesController
  private
  def report_error(error)
    render json: { origin: "override-method", message: error.message }, status: :unprocessable_entity
  end
end
