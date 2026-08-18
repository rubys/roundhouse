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
# Params — narrowing accessors over the recursive request-params tree
# (`Roundhouse::ParamValue`). The synthesized `<Resource>Params.from_raw`
# calls these instead of open-coding `is_a?` narrowing per field, so the
# type test lives in one transpiled body rather than in generated code
# whose shape each emitter has to recognize.
require_relative "runtime/params"
# ActionText::Content — the coder behind a `has_rich_text` attribute.
# The RichText RECORD is an ordinary lowered model (it has a table);
# this is only the value its `body` column reads back as, so it loads
# with the other value classes rather than with the models.
require_relative "runtime/action_text"
require_relative "runtime/importmap"
# ActiveSupport::Duration value class — the emit grounds `70.days` etc.
# to `ActiveSupport::Duration.days(70)`, so the class must be loadable
# on every tree (the CRuby overlay swaps in its Time-reopen-augmented
# sibling at the same path).
require_relative "runtime/active_support_duration"
# Blank-predicate helper for receivers `src/lower/blank.rs` had no static
# type to ground on. Before anything that can hold a `present?` site.
require_relative "runtime/active_support_ext"
require_relative "runtime/rails"
# Park RAILS_ENV where the typed runtime can read it (`Rails.env`
# defaults to development when unset).
Rails.env_name = ENV["RAILS_ENV"]
# The key every signed message derives from (signed cookies, signed ids).
# Read here rather than in the framework runtime for the same reason
# RAILS_ENV is: the runtime typing gate doesn't model `ENV[]`.
Rails.secret_key_base = ENV["SECRET_KEY_BASE"]
# Per-app Rails::Application reopen — the app's real config methods
# (`Rails.application.name` in layouts). Emitted unconditionally (a
# stub reopen when the source app has none); loads right after the
# runtime shim it reopens.
require_relative "config/application"
# Pin the process zone to the app's config.time_zone before anything
# renders. Rails presents every AR temporal value in that zone
# REGARDLESS of the host's — `parse_db_time` hydrates the stored UTC
# instant and lands it here with `.getlocal`, so strftime/iso8601/pubDate
# offsets match Rails on any host. `config_time_zone` is a framework
# default in runtime/rails.rb that the app's reopen (required just above)
# overrides when ingest found a `config.time_zone` line, so this call
# resolves statically — no respond_to? guard, which the strict target
# could not take anyway. Twin of the overlay main.rb's pin.
ENV["TZ"] = ActiveSupport::RAILS_TZ_TO_IANA.fetch(
  Rails.application.config_time_zone, Rails.application.config_time_zone
)
require_relative "runtime/active_record"
require_relative "config/schema"
require_relative "runtime/action_dispatch"
# Typed Request value object (remote_ip / referer / xhr? / env bag) —
# spinel-tree only; the CRuby overlay keeps its CGI-env-backed Request
# at runtime/action_dispatch_request.rb and the two shapes must not
# blend.
require_relative "runtime/action_dispatch/request"
require_relative "runtime/action_controller"
# typed_store virtual-attribute seam (flat-YAML subset on this tree;
# the CRuby overlay swaps in its real-YAML sibling at the same path).
# Before app/models — the synthesized settings accessors route
# through it.
require_relative "runtime/typed_store"
# has_json (ActiveModel::SchematizedJson) virtual-attribute seam — the
# JSON sibling of the line above. After runtime/json_builder, whose
# string escaper it uses.
require_relative "runtime/schematized_json"
require_relative "runtime/broadcasts"
require_relative "runtime/tep/tep"
# Spinel-only CGI shim (escape/unescape_html/parse) — CRuby/JRuby use stdlib
# `require "cgi"`. After tep so `Url` (the percent-encoder CGI.escape routes
# to) is defined.
require_relative "runtime/cgi_spinel"
# Spinel-only ERB::Util shim (html_escape) — CRuby/JRuby get it from the
# stdlib Rails loads. After action_controller, whose require chain defines
# the ActionView::ViewHelpers.html_escape this delegates to.
require_relative "runtime/erb_spinel"
# Action Cable WebSocket glue — the /cable endpoint + the Broadcasts
# transport that fans Turbo Stream fragments out to subscribers. Loaded
# after tep (uses Tep::WebSocket / Scheduler / Broadcast) and broadcasts
# (registers as its transport at boot).
require_relative "runtime/cable"
require_relative "config/routes"
# Per-app Importmap override (generated by Roundhouse from the source
# app's config/importmap.rb). Reopens the fallback module to supply
# the source's actual pins. spinel/master fixed module-reopen with
# same-name cmeth dispatch (matz/spinel#517), so this is now a plain
# require_relative under both CRuby and spinel.
require_relative "config/importmap"
# The app/models.rb aggregator (generated — see apply_models_aggregator)
# loads every model/support class. Model files only require their own
# LOAD-time deps (superclass, class-body consts); method-body references
# between them count on this line — and spinel-AOT's static require
# graph reaches every model file through it.
require_relative "app/models"
require_relative "app/views"
# Session-backed CSRF token — reopens ViewHelpers, so it must load
# AFTER everything that defines the shared empty-string default (the
# same ordering contract as the CRuby overlay's
# action_controller_session require).
require_relative "runtime/csrf_token"

