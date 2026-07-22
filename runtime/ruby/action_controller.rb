require_relative "action_controller/base"
# Per-request context statics (Current.request / .controller) + the
# controller's `request` accessor — a reopen file outside the strict-
# target tables (base.rb transpiles everywhere; a Request-typed field
# must not).
require_relative "action_controller/current"
