class HtmlStatusController < ActionController::Base
  before_action { render status: :forbidden }
  before_action { raise "later callback ran" }
  def index
    raise "action ran"
  end
end
