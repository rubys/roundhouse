class TemplatesController < ActionController::Base
  before_action { render template: "templates/blocked" }
  def index
    raise "action ran"
  end
end
