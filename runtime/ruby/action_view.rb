require_relative "action_view/slots"
require_relative "action_view/missing_template"
require_relative "action_view/view_helpers"
# Ruby-family-only reopen (off the strict-target tables) — must load
# after the base module it reopens.
require_relative "action_view/view_helpers_ext"
