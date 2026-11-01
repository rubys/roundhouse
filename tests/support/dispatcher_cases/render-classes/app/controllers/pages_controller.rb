class PagesController < ActionController::Base
  layout "application"
  before_action { render "pages/blocked", status: :service_unavailable }
  def index
    raise "action ran"
  end
end
