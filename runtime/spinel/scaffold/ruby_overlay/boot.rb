# Boot chain — every `require_relative` the app needs, in the order it
# needs them, and NOTHING that starts anything.
#
# Split out of main.rb so `test/test_helper.rb` can share it. The two
# cannot share main.rb itself: the spinel-AOT main.rb boots the server
# UNCONDITIONALLY (its own comment explains why the
# `__FILE__ == $PROGRAM_NAME` guard cannot work under AOT — `__FILE__`
# is "main.rb" and `$PROGRAM_NAME` is the binary's argv[0]), so a test
# that required it opened `storage/development.sqlite3` and died with
# `sqlite3_open(...) failed (14)` before running an assertion.
#
# ONE owner for the order. The harness previously kept its own
# hand-maintained subset of this list; by the time anyone noticed it was
# 23 files short, and the symptom read as a modeling gap
# (`uninitialized constant ActionText`) rather than as a missing require.
#
require "stringio"
# `Time.parse`/`Time#iso8601` — backs the datetime-column accessor
# coercion the Ruby emitter synthesizes (`apply_datetime_lowering`).
require "time"
# `CGI.escape`/`CGI.parse`/`CGI.unescape_html` — app code (lobsters
# models/extras) reaches stdlib CGI directly; Rails gets it via
# ActiveSupport, the CRuby tree gets it here.
require "cgi"
# stdlib ERB for app code that escapes explicitly (`ERB::Util.html_escape`
# in lobsters' Hat#to_html_label) — the util module, not the templating.
require "erb"
# stdlib BigDecimal for app code doing exact decimal math (lobsters'
# Comment#calculated_confidence — its own comment says the Float
# version accumulates enough error to go out of range, so a Float
# shim is not a substitute). Spinel warns-and-ignores this require;
# BigDecimal call sites remain a compile gap there, tree-shaken off
# the served routes today.
require "bigdecimal"

require_relative "runtime/sqlite_adapter"
# Db primitive surface — backs the lowerer-emitted `_adapter_*`
# methods (Level-3 emit + Phase 1 Arel inline-SELECT expansions).
# Required before active_record so Base.rb's default `_adapter_*`
# helpers and per-model overrides find `Db` at constant-resolution
# time. See project_arel_compile_time_first.md.
require_relative "runtime/db"
# Base64 + JSON + Importmap shims. All required before any framework
# Ruby file that references them so spinel-AOT's static resolver
# sees the constants. The per-app config/importmap.rb (when emitted)
# reopens Importmap with the source-derived pins/entry; Base64 and
# JSON have no per-app override. Under CRuby these shims override
# the stdlib equivalents with semantically-identical implementations
# for the surface framework Ruby actually uses.
require_relative "runtime/base64"
require_relative "runtime/json_impl"
# JsonBuilder — the JSON encoding primitives the Jbuilder lowerer
# emits calls to (`Views::Articles.article_json` etc.). Separate from
# `runtime/json.rb`'s `JSON.generate` shim: this module exposes
# `JsonBuilder.encode_value` / `encode_string` for per-value encoding.
require_relative "runtime/json_builder"
# Params — see the spinel scaffold's main.rb for the rationale. Both
# entry points need it: the CRuby target uses this overlay, and patching
# only the spinel one leaves `<Resource>Params.from_raw` reaching an
# undefined constant on every request that carries params.
require_relative "runtime/params"
# ActionText::Content — see the spinel scaffold's main.rb. Both entry
# points need it: a `has_rich_text` model's `body` reader constructs
# one on every read, so an unloaded constant is a NameError on the
# first message render, not a lazy failure.
require_relative "runtime/action_text"
require_relative "runtime/importmap"
require_relative "runtime/rails"
# `GlobalID::Locator` — the READ side of the gid `runtime/rails.rb` mints
# one line up. A channel authorizing a subscribe turns the stream name
# back into a record through it; the two halves live apart because only
# the mint prices every target (see the file's own header).
require_relative "runtime/global_id_locator"
# Park RAILS_ENV where the typed runtime can read it (`Rails.env`
# defaults to development when unset — pass RAILS_ENV=production for
# serving/bench postures; lobsters gates dev-only filters on it).
Rails.env_name = ENV["RAILS_ENV"]
# The key every signed message derives from (signed cookies, signed ids).
# Read here rather than in the framework runtime for the same reason
# RAILS_ENV is: the runtime typing gate doesn't model `ENV[]`.
Rails.secret_key_base = ENV["SECRET_KEY_BASE"]
# Real in-mem cache store behind Rails.cache (CRuby-only; the shared
# runtime's Cache is a recompute-every-fetch no-op).
require_relative "runtime/rails_cache"
require_relative "runtime/rails_application_routes"
require_relative "runtime/active_support_duration"
require_relative "runtime/active_support_time_parsing"
require_relative "runtime/active_support_core_ext"
# Blank-predicate helper for receivers `src/lower/blank.rs` had no static
# type to ground on. CRuby could serve those sites through the core_ext
# reopen above; it now takes the same lowered call every other
# ruby-family target does, so the two stay one behaviour.
require_relative "runtime/active_support_ext"
require_relative "runtime/action_view_number_helper"
require_relative "runtime/action_view_form_builder_extras"
require_relative "runtime/active_record"
require_relative "runtime/active_record_bang"
require_relative "runtime/active_record_serialization"
require_relative "runtime/active_record_relation_ext"
require_relative "config/schema"
require_relative "runtime/action_dispatch"
require_relative "runtime/action_controller"
# After action_controller: its require chain loads the shared
# action_view/view_helpers, and the safe-buffer overrides must win
# that reopen (same ordering contract as action_controller_session's
# form_authenticity_token override below). url_for rides the same
# contract: the shared view_helpers now defines the typed
# String-identity case, and this overlay's polymorphic is_a? version
# must redefine it for CRuby's residual dynamic sites.
require_relative "runtime/action_view_safe_buffer"
# `sanitize` / `strip_tags` / `auto_link` on the REAL rails-html-sanitizer
# (guarded — an app that never sanitizes boots without the gem, and the
# shared runtime's scanner stands). AFTER the safe buffer: these return
# SafeString, which that file's `html_escape` is the reader of.
require_relative "runtime/action_view_sanitize"
require_relative "runtime/action_view_url_for"
# `ActionView::RecordIdentifier.dom_id` — Rails' own home for `dom_id`,
# which an app's test helper can name directly. Overlay-only: the
# targets with no module system flatten it onto `ViewHelpers` and
# collide with the delegate.
require_relative "runtime/action_view_record_identifier"
require_relative "runtime/action_view_missing_template"
require_relative "runtime/action_dispatch_request"
require_relative "runtime/action_controller_session"
require_relative "runtime/action_controller_json_render"
require_relative "runtime/typed_store"
# has_json (ActiveModel::SchematizedJson) virtual-attribute seam — the
# JSON sibling of the line above. After runtime/json_builder, whose
# string escaper it uses.
require_relative "runtime/schematized_json"
require_relative "runtime/action_mailer"
# App-code gem dependencies, guarded so apps that don't use them (the
# blog) boot without the gems installed. The list itself lives in
# runtime/gem_facades.rb — on a ruby-family tree that file IS the
# guarded-require block (project.rs rewrites the spinel façades away),
# and it is also the anchor every emitted class that names a gem
# constant already requires. This line used to be a second copy of the
# list, and the copy had drifted: it was missing rqrcode and
# sentry-ruby, so a gem declared in the Gemfile still went unloaded at
# server boot unless some model happened to pull the anchor in.
require_relative "runtime/gem_facades"
require_relative "runtime/broadcasts"
# `Turbo::StreamsChannel` — the channel a `<turbo-cable-stream-source>`
# names, AND the `broadcast_*_to` class methods a model's after_commit
# reaches (and an app's own tests mock). One constant, both halves, the
# way turbo-rails ships it. After action_cable, whose `Channel::Base` it
# subclasses, and after broadcasts, whose `record` it calls.
require_relative "runtime/action_cable"
require_relative "runtime/turbo_streams"
require_relative "runtime/cgi_io"
require_relative "config/routes"
# Per-app Importmap override (generated by Roundhouse from the source
# app's config/importmap.rb). Conditional because source apps without
# an importmap don't have this file emitted; the runtime/importmap.rb
# fallback stands in that case. The `begin/rescue` is CRuby-style
# error handling — spinel's static analyzer ignores the rescue path,
# which is fine here because Importmap is already defined by the
# fallback require above.
begin
  require_relative "config/importmap"
rescue LoadError
end
# Per-app Rails::Application reopen (generated from the source app's
# config/application.rb) — real config methods (`read_only?`, `name`)
# reached via `Rails.application`. Conditional like the importmap:
# source apps without one fall back to the empty runtime shim class.
begin
  require_relative "config/application"
rescue LoadError
end
# Pin the process zone to the app's config.time_zone (ingest
# synthesizes `config_time_zone` from the one config-DSL line the
# render layer must honor; absent → "UTC", Rails' default). Rails
# presents every AR temporal value in this zone REGARDLESS of the
# host's zone — parse_db_time hydrates UTC then `.getlocal` lands
# here, so strftime/iso8601/pubDate offsets match Rails on any host.
tz_name = if Rails.application.respond_to?(:config_time_zone)
  Rails.application.config_time_zone
else
  "UTC"
end
ENV["TZ"] = ActiveSupport::RAILS_TZ_TO_IANA.fetch(tz_name, tz_name)
# The app/models.rb aggregator (generated — see apply_models_aggregator)
# loads every model/support class. Model files only require their own
# LOAD-time deps (superclass, class-body consts); method-body references
# between them count on this line having run before any dispatch.
require_relative "app/models"
require_relative "app/views"

