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
