class NamedPagesController < ActionController::Base
  before_action :deny
  def index
    raise "action ran"
  end
  private
  def deny
    render template: "pages/blocked", layout: false, status: :forbidden
  end
end
