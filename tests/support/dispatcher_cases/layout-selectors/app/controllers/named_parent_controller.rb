class NamedParentController < ActionController::Base
  before_action :stop
  def index
    raise "action ran"
  end
  private
  def stop
    render template: "shared/context", layout: false, status: :accepted
  end
end
