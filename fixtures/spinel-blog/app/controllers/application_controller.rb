require_relative "../../runtime/action_controller"

# Application-level abstract base; mirrors real-blog's
# ApplicationController. Currently empty — anywhere app-wide policies
# (auth, locale, exception handling) would live if real-blog had any.
class ApplicationController < ActionController::Base
end
