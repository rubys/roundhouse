class GuardedContextController < ActionController::Base
  before_action only: :index, if: -> { action_name == "index" } do
    render template: "shared/context", layout: false
  end
  def index
    raise "action ran"
  end
end
