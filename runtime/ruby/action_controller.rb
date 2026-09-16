require_relative "action_controller/base"
# Per-request context statics (Current.request / .controller) + the
# controller's `request` accessor — a reopen file outside the strict-
# target tables (base.rb transpiles everywhere; a Request-typed field
# must not).
require_relative "action_controller/current"
# Controller-level `cookies` CookieJar — another reopen outside the
# strict-target tables (a CookieJar-typed field must not transpile to
# targets that don't exercise cookies). One typed impl for ruby/jruby/
# spinel, replacing the former CRuby-only overlay CookieJar.
# The keyed-digest primitive the verifier below calls. Required HERE
# rather than from the scaffold's main.rb so the dependency travels with
# its consumer — the framework runtime is copied as a unit by callers
# (the toolchain tests hand-list what they stage), and a require that
# lives in the entry point instead leaves those lists to guess.
require_relative "message_digest"
require_relative "action_controller/message_verifier"
require_relative "action_controller/cookies"
# The error `Params.require_key` raises; travels with its consumer for
# the same reason message_digest does.
require_relative "action_controller/parameter_missing"
# geared_pagination's `set_page_and_extract_portion_from` + the `Page` it
# sets — a third Base reopen kept off the strict-target tables, for the
# reason its own header gives.
require_relative "action_controller/pagination"
# `allow_browser`'s gate — the parsed User-Agent against the app's
# version floors. Called from the filter the lowering synthesizes.
require_relative "action_controller/browser_blocker"
# `rate_limit`'s counter — the window's count against its cap. Called
# from the filter method the lowering synthesizes.
require_relative "action_controller/rate_limiter"
