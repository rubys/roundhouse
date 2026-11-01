class OverrideDeclarationController < WithRescuesController
  rescue_from ArgumentError do |error|
    render json: { origin: "override-declaration", message: error.message }, status: :bad_request
  end
end
