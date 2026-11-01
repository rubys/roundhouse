class OtherPagesController < ActionController::Base
  before_action { render template: "pages/blocked", layout: "other", status: :forbidden }
  def index
    raise "action ran"
  end
end
