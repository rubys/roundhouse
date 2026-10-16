# frozen_string_literal: true
require_relative "action_dispatch/router"
# Whether a render answers `Vary: Accept` (ruby-family dispatchers).
require_relative "action_dispatch/mime_negotiation"
require_relative "action_dispatch/flash"
require_relative "action_dispatch/session"
require_relative "action_dispatch/exceptions"
# X-Request-Id and X-Runtime (ActionDispatch::RequestId, Rack::Runtime),
# stamped by both ruby-family dispatchers around every app response.
require_relative "action_dispatch/request_id"
