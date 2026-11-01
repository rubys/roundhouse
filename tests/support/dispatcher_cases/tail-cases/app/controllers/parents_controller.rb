class ParentsController < ActionController::Base
  def index
    response.set_header("X-Step", "parent")
    render plain: "parent response", status: :accepted
  end
end
