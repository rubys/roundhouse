class HtmlImplicitController < ActionController::Base
  before_action { response.set_header("X-Before", "ran") }
  after_action { response.set_header("X-Observed", response.body) }
  def index
    @title = "implicit body"
  end
end
