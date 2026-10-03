# ActionController::API — the base `rails new --api` gives
# ApplicationController. Rails builds it as Base without the browser
# modules (cookies, flash, request forgery protection, views and
# layouts). Here it is Base, so an API controller has every writer the
# dispatchers call before an action runs (`params=`, `request=`,
# `cookies=`, `flash=`), and the browser modules Rails leaves out stay
# within reach. Without the class, `ActionController::API` was a
# NameError on the ruby tree, and every request to the spinel binary
# answered 500 (`undefined method 'params='`, #163).
#
# Ruby-family home (off the strict-target tables), like the other files
# `action_controller.rb` requires. base.rb transpiles to every target;
# what an `API` class would take on each of those is a separate
# question from the one this file answers.
module ActionController
  class API < Base
  end
end
