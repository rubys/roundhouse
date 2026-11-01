class BlockStopsController < ApplicationController
  before_action { render json: { origin: "before" }, status: :forbidden }
  before_action { raise "later callback ran" }
  after_action { response.set_header("X-After", "ran") }
  def index
    raise "action ran"
  end
end
