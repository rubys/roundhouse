class BlockParentController < ActionController::Base
  before_action { render template: "shared/context", layout: false, status: :forbidden }
  def index
    raise "action ran"
  end
end
