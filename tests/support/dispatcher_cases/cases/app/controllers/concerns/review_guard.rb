module ReviewGuard
  extend ActiveSupport::Concern
  included do
    before_action { render json: { origin: "concern" }, status: :unauthorized }
  end
end
