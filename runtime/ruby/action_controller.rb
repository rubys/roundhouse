# frozen_string_literal: true
require_relative "action_controller/base"
# `ControllerConfig` and the class-side `config`, the same object
# `Base#config` (instance_config.rb) returns. Both off the strict-target
# tables; two files because the typed-runtime gate cannot hold two
# methods named `config` in one signature table.
require_relative "action_controller/controller_config"
require_relative "action_controller/instance_config"
# `ActionController::API`, the base an API app's ApplicationController
# names: Base under the API name.
require_relative "action_controller/api"
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
# `cookies.encrypted` and the session store: the verifier's envelope
# under AES-256-GCM rather than an HMAC.
require_relative "action_controller/message_encryptor"
# Masked CSRF tokens + verify. After Current (session) and the
# digest primitives it consumes.
require_relative "action_controller/authenticity_token"
require_relative "action_controller/cookies"
# `response.cache_control`'s Hash-like surface (`[]`, `[]=`, `delete`,
# `merge!`, `replace`) + `commit_cache_control!` — another reopen
# outside the strict-target tables, for the reason its own header
# gives (the typed `CacheControlStore` class lives in base.rb and
# transpiles everywhere; this Hash-subscript surface over it does
# not).
require_relative "action_controller/cache_control"
# Rack::ETag and Rack::ConditionalGet over the buffered response
# (`commit_conditional_get!`), the step after `commit_cache_control!`
# in both wire paths. Ruby-family only, as the file above.
require_relative "action_controller/conditional_get"
# The error `Params.require_key` raises; travels with its consumer for
# the same reason message_digest does.
require_relative "action_controller/parameter_missing"
# geared_pagination's `set_page_and_extract_portion_from` + the `Page` it
# sets — a third Base reopen kept off the strict-target tables, for the
# reason its own header gives.
require_relative "action_controller/pagination"
# The pagy gem's `pagy` + the `Pagy` paginator it answers -- a fourth
# Base reopen kept off the strict-target tables; project.rs drops it
# (and this require) unless the app's Gemfile.lock carries pagy.
require_relative "action_controller/pagy"
# `allow_browser`'s gate — the parsed User-Agent against the app's
# version floors. Called from the filter the lowering synthesizes.
require_relative "action_controller/browser_blocker"
# `rate_limit`'s counter — the window's count against its cap. Called
# from the filter method the lowering synthesizes.
require_relative "action_controller/rate_limiter"
require_relative "action_controller/inertia"
# `invisible_captcha`'s honeypot gate — called from the filter method
# the lowering synthesizes.
require_relative "action_controller/invisible_captcha"
