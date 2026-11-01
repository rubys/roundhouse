class ImplicitContextController < ActionController::Base
  before_action { response.set_header("X-Step", "before") }
  def index
    @message = "implicit"
  end
end
